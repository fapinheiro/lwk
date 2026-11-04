//! Sponsor UTXO logic for the FeeSponsor feature.
//!
//! Exposes two methods on [`crate::Wollet`]:
//! - [`crate::Wollet::utxo_secrets`]: returns blinding factors and metadata for a specific UTXO
//!   owned by this wallet so it can be used as an external UTXO in a PSET built by another party.
//! - [`crate::Wollet::sponsor_prepare`]: validates a sponsor PSET (security rules) and applies
//!   [`crate::Wollet::add_details`] so the sponsor signer can co-sign the LBTC input.

use elements::{
    confidential::{AssetBlindingFactor, ValueBlindingFactor},
    pset::PartiallySignedTransaction,
    AssetId, OutPoint, Script,
};

use crate::{error::Error, pset_create::validate_address, wollet::Wollet};

/// Blinding secrets and metadata for a specific unspent UTXO owned by this wallet.
///
/// Used to build an `ExternalUtxo` on the app side via `lwk_bindings`.
#[derive(Debug)]
pub struct UtxoSecrets {
    /// Transaction ID of the origin transaction.
    pub txid: elements::Txid,

    /// Output index within the origin transaction.
    pub vout: u32,

    /// Full origin transaction serialized as hex.
    pub tx_hex: String,

    /// Script pubkey of the output.
    pub script_pubkey: Script,

    /// Asset ID.
    pub asset: AssetId,

    /// Asset blinding factor.
    pub asset_bf: AssetBlindingFactor,

    /// Value in satoshi.
    pub value: u64,

    /// Value blinding factor.
    pub value_bf: ValueBlindingFactor,

    /// Block height at which the UTXO was confirmed, or `None` if unconfirmed.
    pub height: Option<u32>,

    /// Maximum weight of the input script witness needed to satisfy this output.
    pub max_weight_to_satisfy: usize,

    /// Whether the wallet descriptor produces segwit outputs.
    pub is_segwit: bool,
}

/// Classification of a single PSET output from the sponsor wallet's perspective.
#[derive(Debug)]
pub struct SponsorOutput {
    /// Output index (0-based).
    pub index: usize,

    /// Script pubkey of the output.
    pub script_pubkey: Script,

    /// How this output is classified.
    pub kind: SponsorOutputKind,

    /// For `deposit` outputs only: whether the PSET `blinding_key` field matches the blinding
    /// pubkey of `deposit_address`.  `None` if the field is absent in the PSET output.
    pub blinding_key_matches: Option<bool>,
}

/// Classification of a PSET output from the sponsor's perspective.
#[derive(Debug)]
pub enum SponsorOutputKind {
    /// Output with an empty script pubkey (network fee).
    Fee,
    /// Output whose script pubkey equals the `deposit_address` passed to `sponsor_prepare`.
    Deposit,
    /// Output whose script pubkey belongs to this (sponsor) wallet.
    Mine,
    /// Any other output.
    Other,
}

/// Result returned by [`crate::Wollet::sponsor_prepare`].
#[derive(Debug)]
pub struct SponsorPrepared {
    /// The PSET after `add_details` has been applied.
    pub pset: PartiallySignedTransaction,

    /// Transaction ID of the unsigned transaction embedded in the PSET.
    pub txid: elements::Txid,

    /// Total fee in satoshi (sum of all fee outputs).
    pub fee: u64,

    /// Classified outputs.
    pub outputs: Vec<SponsorOutput>,
}

impl Wollet {
    /// Return blinding secrets for a specific unspent UTXO owned by this wallet.
    ///
    /// Returns [`Error::SponsorUtxoNotFound`] if `outpoint` is not an unspent UTXO of the wallet.
    pub fn utxo_secrets(&self, outpoint: OutPoint) -> Result<UtxoSecrets, Error> {
        // The outpoint must be an unspent output of this wallet.
        let utxo = self
            .utxos()?
            .into_iter()
            .find(|u| u.outpoint == outpoint)
            .ok_or(Error::SponsorUtxoNotFound)?;

        let tx = self
            .cache
            .tx(&outpoint.txid)
            .ok_or(Error::MissingTransaction)?;

        let tx_hex = elements::encode::serialize_hex(&tx);

        Ok(UtxoSecrets {
            txid: outpoint.txid,
            vout: outpoint.vout,
            tx_hex,
            script_pubkey: utxo.script_pubkey,
            asset: utxo.unblinded.asset,
            asset_bf: utxo.unblinded.asset_bf,
            value: utxo.unblinded.value,
            value_bf: utxo.unblinded.value_bf,
            height: utxo.height,
            max_weight_to_satisfy: self.max_weight_to_satisfy(),
            is_segwit: self.is_segwit(),
        })
    }

    /// Validate and enrich a sponsor PSET.
    ///
    /// Applies the following security rules **in order** (first failure returns an error):
    ///
    /// 1. `deposit_address` parses correctly and belongs to this wallet's network.
    /// 2. The set of PSET inputs whose `witness_utxo.script_pubkey` belongs to this wallet is
    ///    exactly `{outpoint}` (no other wallet inputs, the reserved one is present).
    /// 3. The `sighash_type` of that input is absent or `SIGHASH_ALL`.
    /// 4. The `witness_utxo` of that input is identical to the real `TxOut` stored in the wallet.
    /// 5. That input has no issuance or reissuance (`has_issuance()` false and all PSET issuance
    ///    fields empty).
    /// 6. [`Wollet::add_details`] is applied to enrich the PSET for signing.
    /// 7. Outputs are classified and returned together with txid and fee.
    pub fn sponsor_prepare(
        &self,
        pset: &mut PartiallySignedTransaction,
        outpoint: OutPoint,
        deposit_address: &str,
    ) -> Result<SponsorPrepared, Error> {
        use elements::EcdsaSighashType;

        // Rule 1: validate deposit_address for this network.
        let deposit_addr = validate_address(deposit_address, self.network())
            .map_err(|_| Error::SponsorDepositAddressInvalid)?;
        let deposit_spk = deposit_addr.script_pubkey();

        // Rule 2: the wallet-owned inputs in the PSET must be exactly {outpoint}.
        //
        // An input "belongs to the wallet" if its witness_utxo.script_pubkey is in the wallet's
        // known script paths.
        let wallet_inputs: Vec<(usize, OutPoint)> = pset
            .inputs()
            .iter()
            .enumerate()
            .filter_map(|(i, inp)| {
                let spk = inp.witness_utxo.as_ref().map(|txout| &txout.script_pubkey)?;
                if self.cache.paths.contains_key(spk) {
                    let op =
                        OutPoint::new(inp.previous_txid, inp.previous_output_index);
                    Some((i, op))
                } else {
                    None
                }
            })
            .collect();

        if wallet_inputs.len() != 1 || wallet_inputs[0].1 != outpoint {
            return Err(Error::SponsorInputMismatch);
        }
        let sponsor_inp_idx = wallet_inputs[0].0;

        // Keep an immutable borrow scoped so we can call extract_tx later.
        {
            let sponsor_inp = &pset.inputs()[sponsor_inp_idx];

            // Rule 3: sighash_type must be absent or SIGHASH_ALL.
            if let Some(sighash) = sponsor_inp.sighash_type {
                match sighash.ecdsa_hash_ty() {
                    Some(EcdsaSighashType::All) => {}
                    _ => return Err(Error::SponsorSighashNotAll),
                }
            }

            // Rule 4: witness_utxo must be present and match the actual TxOut.
            let actual_txout = {
                let tx = self
                    .cache
                    .tx(&outpoint.txid)
                    .ok_or(Error::MissingTransaction)?;
                tx.output
                    .get(outpoint.vout as usize)
                    .ok_or(Error::MissingVout)?
                    .clone()
            };
            match &sponsor_inp.witness_utxo {
                Some(w) if *w == actual_txout => {}
                _ => return Err(Error::SponsorWitnessUtxoMismatch),
            }

            // Rule 5: no issuance on this input (PSET-level fields).
            if sponsor_inp.issuance_value_amount.is_some()
                || sponsor_inp.issuance_inflation_keys.is_some()
                || sponsor_inp.issuance_asset_entropy.is_some()
                || sponsor_inp.issuance_blinding_nonce.is_some()
            {
                return Err(Error::SponsorIssuanceNotAllowed);
            }
        }

        // Rule 5 (continued): check has_issuance() on the extracted TxIn.
        let pre_tx = pset.extract_tx()?;
        if pre_tx
            .input
            .get(sponsor_inp_idx)
            .map(|i| i.has_issuance())
            .unwrap_or(false)
        {
            return Err(Error::SponsorIssuanceNotAllowed);
        }

        // Rule 6: apply add_details so the sponsor signer can populate bip32_derivation.
        self.add_details(pset)?;

        // Rule 7: classify outputs and compute fee from the (now-enriched) PSET.
        let tx = pset.extract_tx()?;
        let txid = tx.txid();

        let fee: u64 = tx
            .output
            .iter()
            .filter(|o| o.script_pubkey.is_empty())
            .map(|o| o.value.explicit().unwrap_or(0))
            .sum();

        let outputs = pset
            .outputs()
            .iter()
            .enumerate()
            .zip(tx.output.iter())
            .map(|((idx, pset_out), txout)| {
                let spk = &txout.script_pubkey;
                let kind = if spk.is_empty() {
                    SponsorOutputKind::Fee
                } else if *spk == deposit_spk {
                    SponsorOutputKind::Deposit
                } else if self.cache.paths.contains_key(spk) {
                    SponsorOutputKind::Mine
                } else {
                    SponsorOutputKind::Other
                };

                // blinding_key_matches is only meaningful for Deposit outputs.
                let blinding_key_matches = match kind {
                    SponsorOutputKind::Deposit => pset_out.blinding_key.map(|bk| {
                        let bk_bytes = bk.inner.serialize();
                        // deposit_addr is confidential (validated above), so blinding_pubkey
                        // is always Some here.
                        deposit_addr
                            .blinding_pubkey
                            .map(|k| k.serialize() == bk_bytes)
                            .unwrap_or(false)
                    }),
                    _ => None,
                };

                SponsorOutput {
                    index: idx,
                    script_pubkey: spk.clone(),
                    kind,
                    blinding_key_matches,
                }
            })
            .collect();

        Ok(SponsorPrepared {
            pset: pset.clone(),
            txid,
            fee,
            outputs,
        })
    }
}

#[cfg(test)]
mod tests {
    use elements::{
        bitcoin::bip32::ChildNumber,
        confidential::{Asset, Nonce, Value},
        pset::{Output, PartiallySignedTransaction, PsbtSighashType},
        AssetId, EcdsaSighashType, LockTime, OutPoint, Script, Transaction, TxOut, TxOutWitness,
    };

    use crate::{descriptor::Chain, Network, WolletBuilder, WolletDescriptor};

    use super::SponsorOutputKind;

    // Testnet wallet descriptor used in multiple tests.
    const TESTNET_DESC: &str = "ct(slip77(ab5824f4477b4ebb00a132adfd8eb0b7935cf24f6ac151add5d1913db374ce92),\
        elwpkh([759db348/84'/1'/0']tpubDCRMaF33e44pcJj534LXVhFbHibPbJ5vuLhSSPFAw57kYURv4\
        tzXFL6LSnd78bkjqdmE3USedkbpXJUPA1tdzKfuYSL7PianceqAhwL2UkA/<0;1>/*))#cch6wrnp";

    // A valid testnet confidential address used for deposit_address in tests.
    const TESTNET_CONF_ADDR: &str =
        "tlq1qq2xvpcvfup5j8zscjq05u2wxxjcyewk7979f3mmz5l7uw5pqmx6xf5xy50hsn6vhkm5euwt72x878eq6zxx2z58hd7zrsg9qn";

    fn testnet_wollet() -> crate::Wollet {
        let desc: WolletDescriptor = TESTNET_DESC.parse().expect("desc");
        WolletBuilder::new(Network::TestnetLiquid, desc)
            .build()
            .expect("wollet")
    }

    fn fake_outpoint() -> OutPoint {
        let txid = "0101010101010101010101010101010101010101010101010101010101010101"
            .parse()
            .expect("txid");
        OutPoint::new(txid, 0)
    }

    fn explicit_txout(script_pubkey: Script) -> TxOut {
        TxOut {
            asset: Asset::Explicit(AssetId::default()),
            value: Value::Explicit(10_000),
            nonce: Nonce::Null,
            script_pubkey,
            witness: TxOutWitness::default(),
        }
    }

    /// Set up a wollet with:
    /// - Script at (External, 0) inserted into `cache.paths`.
    /// - A transaction at `outpoint` stored in `cache`, with that script's txout at vout 0.
    fn wollet_with_known_input(outpoint: OutPoint) -> (crate::Wollet, Script, TxOut) {
        let mut wollet = testnet_wollet();
        let desc = wollet.descriptor.clone();
        let (script, _, _) = wollet
            .cache
            .get_or_derive(Chain::External, ChildNumber::from_normal_idx(0).unwrap(), &desc)
            .expect("derive");
        wollet.cache.paths.insert(
            script.clone(),
            (Chain::External, ChildNumber::from_normal_idx(0).unwrap()),
        );
        let txout = explicit_txout(script.clone());
        let tx = Transaction {
            version: 2,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![txout.clone()],
        };
        wollet
            .cache
            .update(
                &[(outpoint.txid, None)],
                &[],
                &[(outpoint.txid, tx)],
                false,
                vec![],
            )
            .expect("cache update");
        (wollet, script, txout)
    }

    /// Build a PSET with one input pointing to `outpoint` using `witness_utxo`.
    fn pset_with_input(outpoint: OutPoint, witness_utxo: TxOut) -> PartiallySignedTransaction {
        let mut pset = PartiallySignedTransaction::default();
        let mut input = elements::pset::Input::from_prevout(outpoint);
        input.witness_utxo = Some(witness_utxo);
        pset.add_input(input);
        pset
    }

    // ── utxo_secrets ─────────────────────────────────────────────────────────

    #[test]
    fn utxo_secrets_not_found() {
        let wollet = testnet_wollet();
        let op = OutPoint::default();
        let err = wollet.utxo_secrets(op).unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_UTXO_NOT_FOUND"),
            "unexpected: {err}"
        );
    }

    // ── sponsor_prepare error paths ───────────────────────────────────────────

    #[test]
    fn sponsor_prepare_deposit_address_invalid_wrong_network() {
        let wollet = testnet_wollet();
        let op = OutPoint::default();
        // A mainnet confidential address on a testnet wallet must fail.
        let mainnet_addr =
            "lq1qqvxk052kf3qtkxmrakx50a9gc3smqad2ync54hzntjt980kfej9kkfe0247rp5h4yzmdftsahhw64uy8pzfe7cpg4fgykm7cv";
        // Address check is the very first rule, so PSET contents don't matter.
        let mut pset = PartiallySignedTransaction::default();
        let err = wollet
            .sponsor_prepare(&mut pset, op, mainnet_addr)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_DEPOSIT_ADDRESS_INVALID"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn sponsor_prepare_deposit_address_invalid_unconfidential() {
        let wollet = testnet_wollet();
        let op = OutPoint::default();
        // An unconfidential address should also fail (validate_address requires confidential).
        let unconf_addr = "tex1qwrsaxt8ky5zrexqnzlxfpudpvt2j3lxq9fwea3";
        let mut pset = PartiallySignedTransaction::default();
        let err = wollet
            .sponsor_prepare(&mut pset, op, unconf_addr)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_DEPOSIT_ADDRESS_INVALID"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn sponsor_prepare_input_mismatch_empty_pset() {
        let wollet = testnet_wollet();
        let op = OutPoint::default();
        // Empty PSET has no inputs — wallet-owned input set is empty != {op}.
        let mut pset = PartiallySignedTransaction::default();
        let err = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn sponsor_sighash_not_all() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        // Each non-ALL sighash type must trigger SPONSOR_SIGHASH_NOT_ALL (rule 3).
        let bad_sighashes = [
            EcdsaSighashType::None,
            EcdsaSighashType::Single,
            EcdsaSighashType::SinglePlusAnyoneCanPay,
        ];
        for bad in bad_sighashes {
            let mut pset = pset_with_input(op, txout.clone());
            pset.inputs_mut()[0].sighash_type =
                Some(PsbtSighashType::from_u32(bad.as_u32()));
            let err = wollet
                .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
                .unwrap_err();
            assert!(
                err.to_string().starts_with("SPONSOR_SIGHASH_NOT_ALL"),
                "expected SPONSOR_SIGHASH_NOT_ALL for {bad:?}, got: {err}"
            );
        }

        // Absent sighash must pass rule 3 — no SPONSOR_SIGHASH_NOT_ALL error.
        let mut pset = pset_with_input(op, txout.clone());
        let result = wollet.sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR);
        assert!(
            !result
                .err()
                .map(|e| e.to_string().starts_with("SPONSOR_SIGHASH_NOT_ALL"))
                .unwrap_or(false),
            "absent sighash must not produce SPONSOR_SIGHASH_NOT_ALL"
        );

        // Explicit SIGHASH_ALL must also pass rule 3.
        let mut pset = pset_with_input(op, txout.clone());
        pset.inputs_mut()[0].sighash_type =
            Some(PsbtSighashType::from_u32(EcdsaSighashType::All.as_u32()));
        let result = wollet.sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR);
        assert!(
            !result
                .err()
                .map(|e| e.to_string().starts_with("SPONSOR_SIGHASH_NOT_ALL"))
                .unwrap_or(false),
            "explicit SIGHASH_ALL must not produce SPONSOR_SIGHASH_NOT_ALL"
        );
    }

    #[test]
    fn sponsor_witness_utxo_mismatch() {
        let op = fake_outpoint();
        let (wollet, script, _actual_txout) = wollet_with_known_input(op);

        // witness_utxo has the right script but a different value → rule 4 fails.
        let wrong_txout = TxOut {
            asset: Asset::Explicit(AssetId::default()),
            value: Value::Explicit(99_999), // differs from actual 10_000
            nonce: Nonce::Null,
            script_pubkey: script.clone(),
            witness: TxOutWitness::default(),
        };
        let mut pset = pset_with_input(op, wrong_txout);
        let err = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_WITNESS_UTXO_MISMATCH"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn sponsor_issuance_not_allowed() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        // witness_utxo matches the actual txout (rules 2–4 pass),
        // but issuance_value_amount is set → rule 5 fails.
        let mut pset = pset_with_input(op, txout);
        pset.inputs_mut()[0].issuance_value_amount = Some(1);
        let err = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_ISSUANCE_NOT_ALLOWED"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn sponsor_output_classification() {
        let op = fake_outpoint();
        let (mut wollet, _script, txout) = wollet_with_known_input(op);

        // Derive a second script (index 1) for the Mine output.
        let desc = wollet.descriptor.clone();
        let (mine_script, _, _) = wollet
            .cache
            .get_or_derive(Chain::External, ChildNumber::from_normal_idx(1).unwrap(), &desc)
            .expect("derive mine script");
        wollet.cache.paths.insert(
            mine_script.clone(),
            (Chain::External, ChildNumber::from_normal_idx(1).unwrap()),
        );

        // Deposit script pubkey derived from TESTNET_CONF_ADDR.
        let deposit_spk = {
            use elements::Address;
            let addr: Address = TESTNET_CONF_ADDR.parse().expect("parse deposit addr");
            addr.script_pubkey()
        };

        // "Other" output: a script that is not in the wallet and not the deposit address.
        let other_script = Script::from(vec![0x51]); // OP_1

        let policy = wollet.policy_asset();

        // Build PSET: one sponsor input + four outputs (fee, deposit, mine, other).
        let mut pset = pset_with_input(op, txout);
        pset.add_output(Output::new_explicit(Script::default(), 500, policy, None));
        pset.add_output(Output::new_explicit(deposit_spk, 10_000, policy, None));
        pset.add_output(Output::new_explicit(mine_script, 5_000, policy, None));
        pset.add_output(Output::new_explicit(other_script, 3_000, policy, None));

        let result = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare");

        assert_eq!(result.outputs.len(), 4);
        assert_eq!(result.fee, 500);

        assert!(matches!(result.outputs[0].kind, SponsorOutputKind::Fee));
        assert!(matches!(result.outputs[1].kind, SponsorOutputKind::Deposit));
        assert!(matches!(result.outputs[2].kind, SponsorOutputKind::Mine));
        assert!(matches!(result.outputs[3].kind, SponsorOutputKind::Other));
    }
}
