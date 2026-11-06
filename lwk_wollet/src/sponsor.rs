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
    secp256k1_zkp::SecretKey,
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

    /// Private blinding key for the UTXO's script pubkey (slip77 derivation).
    ///
    /// Used by the app to call `TxOut.unblind(key)` and reconstruct the `ExternalUtxo`.
    /// This is the per-script derived key, not the master blinding key; it cannot be
    /// used to derive keys for other scripts.
    pub blinding_key: SecretKey,
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
#[derive(Debug, serde::Serialize)]
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

        let ct_desc = self.descriptor.ct_descriptor()?;
        let blinding_key =
            lwk_common::derive_blinding_key(ct_desc, &utxo.script_pubkey)
                .ok_or(Error::MissingPrivateBlindingKey)?;

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
            blinding_key,
        })
    }

    /// Validate and enrich a sponsor PSET.
    ///
    /// # `blinding_key` in PSET outputs
    ///
    /// When the user wallet builds a PSET with [`TxBuilder::add_recipient`] using a confidential
    /// address, the builder sets `Output::blinding_key` to the recipient's blinding public key.
    /// Therefore, for a deposit output built this way, `blinding_key_matches` in the returned
    /// [`SponsorOutput`] will be `Some(true)` when the address matches and `Some(false)` when it
    /// does not.  `None` is only returned when the PSET output has no `blinding_key` field (e.g.
    /// the fee output or an explicitly-constructed unconfidential output).
    ///
    /// # `txid` stability across signing rounds
    ///
    /// The `txid` returned here is the txid of the **unsigned** transaction embedded in the PSET
    /// (`pset.extract_tx().txid()`).  In Liquid (and Bitcoin) segwit transactions, the txid
    /// commits only to the transaction body (inputs + outputs + locktime) and excludes witness
    /// data.  Signing adds witness data but does not modify the transaction body, so the txid
    /// computed here will remain identical after co-signing by the sponsor and after the user
    /// signs its inputs.  The app can use this txid to reference the transaction before broadcast.
    ///
    /// # Validation rules applied (in order, first failure returns an error):
    ///
    /// 1. `deposit_address` parses correctly and belongs to this wallet's network.
    /// 2. The set of PSET inputs whose outpoint is known to this wallet's cache (i.e., the txid
    ///    is in the cache *and* the output's `script_pubkey` is in `cache.paths`) is exactly
    ///    `{outpoint}`.  Identification is by outpoint—not by `witness_utxo`—so this rule catches
    ///    wallet inputs even when `witness_utxo` is absent.
    /// 3. The `sighash_type` of that input is absent or `SIGHASH_ALL`.
    /// 4. The `witness_utxo` of that input is present and identical to the real `TxOut` stored in
    ///    the wallet.
    /// 5. That input has no issuance or reissuance (`has_issuance()` false and all PSET issuance
    ///    fields empty).
    /// 6. [`Wollet::add_details`] is applied to enrich the PSET for signing.
    /// 7. After `add_details`, no input other than the sponsor input may carry `bip32_derivation`
    ///    entries whose fingerprint matches any signer of this wallet.  This blocks two attack
    ///    vectors: (a) a rogue input that spoofs a wallet `script_pubkey` in `witness_utxo` so
    ///    that `add_details` populates its derivation path, and (b) a rogue input that injects
    ///    `bip32_derivation` entries with the wallet's fingerprint directly.
    /// 8. Outputs are classified and returned together with txid and fee.
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
        // An input "belongs to the wallet" if the transaction it spends is known to this wallet's
        // cache AND the output's script_pubkey is in cache.paths.  We look up by outpoint rather
        // than by witness_utxo so that:
        //   (a) inputs with no witness_utxo are still detected (Rule 4 then rejects them), and
        //   (b) a rogue input cannot fake its way past this rule by spoofing witness_utxo.
        let wallet_inputs: Vec<(usize, OutPoint)> = pset
            .inputs()
            .iter()
            .enumerate()
            .filter_map(|(i, inp)| {
                let op = OutPoint::new(inp.previous_txid, inp.previous_output_index);
                let spk = self
                    .cache
                    .tx(&op.txid)
                    .and_then(|tx| tx.output.get(op.vout as usize).map(|o| o.script_pubkey.clone()))?;
                if self.cache.paths.contains_key(&spk) {
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
            // Note: TxBuilder strips the output witness (rangeproof + surjection_proof)
            // from witness_utxo (see tx_builder.rs:127), so we must clear the witness
            // on the cached TxOut before comparing.
            let actual_txout = {
                let tx = self
                    .cache
                    .tx(&outpoint.txid)
                    .ok_or(Error::MissingTransaction)?;
                let mut txout = tx
                    .output
                    .get(outpoint.vout as usize)
                    .ok_or(Error::MissingVout)?
                    .clone();
                txout.witness = elements::TxOutWitness::default();
                txout
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

        // Rule 7: after add_details, no non-sponsor input may carry bip32_derivation entries
        // whose fingerprint matches any signer of this wallet.
        //
        // This blocks two attack vectors:
        //   (a) A rogue input that presents a wallet script_pubkey in witness_utxo, causing
        //       add_details to populate its bip32_derivation with wallet derivation paths.
        //   (b) A rogue input that injects bip32_derivation entries with the wallet's key
        //       fingerprint, potentially tricking the signer into signing it.
        let signer_fps = self.signers();
        for (i, inp) in pset.inputs().iter().enumerate() {
            if i == sponsor_inp_idx {
                continue;
            }
            if inp
                .bip32_derivation
                .values()
                .any(|(fp, _)| signer_fps.contains(fp))
            {
                return Err(Error::SponsorInputMismatch);
            }
        }

        // Rule 8: classify outputs and compute fee from the (now-enriched) PSET.
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
        // Spec 4.2 explicitly lists NONE, SINGLE, ALL|ANYONECANPAY as failures.
        let bad_sighashes = [
            EcdsaSighashType::None,
            EcdsaSighashType::Single,
            EcdsaSighashType::AllPlusAnyoneCanPay,
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

    // ── G2: witness_utxo absent / extra-input variants ───────────────────────

    /// Sponsor input is in the wallet cache (Rule 2 passes) but has no `witness_utxo`.
    /// Rule 4 must fire → `SPONSOR_WITNESS_UTXO_MISMATCH`.
    ///
    /// Under the old Rule 2 (witness_utxo-based detection) this input was invisible
    /// to the wallet and Rule 2 fired first with `SPONSOR_INPUT_MISMATCH`.
    /// Under the new outpoint-based Rule 2 the input IS detected, so Rule 4 is
    /// reached and produces the correct diagnostic.
    #[test]
    fn sponsor_witness_utxo_absent_rejected() {
        let op = fake_outpoint();
        let (wollet, _script, _txout) = wollet_with_known_input(op);

        // Build a PSET where the sponsor input has the correct outpoint but no witness_utxo.
        let mut pset = PartiallySignedTransaction::default();
        let input = elements::pset::Input::from_prevout(op); // witness_utxo = None
        pset.add_input(input);

        let err = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_WITNESS_UTXO_MISMATCH"),
            "absent witness_utxo must yield SPONSOR_WITNESS_UTXO_MISMATCH, got: {err}"
        );
    }

    /// Two wallet inputs in the PSET, but the second one has **no** `witness_utxo`.
    /// Under the old code the second input was invisible (witness_utxo-based detection).
    /// Under the new outpoint-based Rule 2 it is detected → `SPONSOR_INPUT_MISMATCH`.
    #[test]
    fn sponsor_extra_wallet_input_no_witness_utxo() {
        let op0 = fake_outpoint();
        let (mut wollet, _script0, txout0) = wollet_with_known_input(op0);

        // Register a second wallet script+tx in the cache.
        let desc = wollet.descriptor.clone();
        let (script1, _, _) = wollet
            .cache
            .get_or_derive(Chain::External, ChildNumber::from_normal_idx(1).unwrap(), &desc)
            .expect("derive script1");
        wollet.cache.paths.insert(
            script1.clone(),
            (Chain::External, ChildNumber::from_normal_idx(1).unwrap()),
        );
        let txout1 = explicit_txout(script1);
        let op1_txid = "0202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .expect("txid");
        let op1 = OutPoint::new(op1_txid, 0);
        let tx1 = Transaction {
            version: 2,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![txout1],
        };
        wollet
            .cache
            .update(&[(op1.txid, None)], &[], &[(op1.txid, tx1)], false, vec![])
            .expect("cache update for op1");

        // Build PSET: op0 with witness_utxo, op1 without witness_utxo.
        let mut pset = pset_with_input(op0, txout0);
        let input1 = elements::pset::Input::from_prevout(op1); // no witness_utxo
        pset.add_input(input1);

        // Both op0 and op1 are found in the wallet cache → wallet_inputs.len() == 2.
        let err = wollet
            .sponsor_prepare(&mut pset, op0, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
            "unexpected: {err}"
        );
    }

    /// An external input whose outpoint is NOT in the wallet cache, but whose
    /// `witness_utxo.script_pubkey` is a wallet script.  Rule 2 (cache-based) does not
    /// detect it, but `add_details` populates `bip32_derivation` on it → Rule 7 fires.
    #[test]
    fn sponsor_extra_input_fake_wallet_script() {
        let op0 = fake_outpoint();
        let (wollet, script0, txout0) = wollet_with_known_input(op0);

        // op_ext: a fresh outpoint that is NOT in the wallet cache.
        let ext_txid = "0404040404040404040404040404040404040404040404040404040404040404"
            .parse()
            .expect("txid");
        let op_ext = OutPoint::new(ext_txid, 0);

        // The rogue input uses the wallet's real script_pubkey in witness_utxo so that
        // add_details will populate bip32_derivation on it.
        let txout_fake = explicit_txout(script0);
        let mut ext_input = elements::pset::Input::from_prevout(op_ext);
        ext_input.witness_utxo = Some(txout_fake);

        let mut pset = pset_with_input(op0, txout0);
        pset.add_input(ext_input);

        // Rule 2 passes (only op0 is in cache).
        // Rule 4 passes (op0's witness_utxo matches).
        // add_details populates bip32_derivation on op_ext (fake wallet script).
        // Rule 7 rejects it.
        let err = wollet
            .sponsor_prepare(&mut pset, op0, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
            "unexpected: {err}"
        );
    }

    /// An external input with no wallet connection at all, but `bip32_derivation` was
    /// manually pre-populated with the wallet's key fingerprint.
    /// Rule 7 (post-add_details bip32_derivation guard) must catch it.
    #[test]
    fn sponsor_extra_input_bip32_derivation_injected() {
        use elements::bitcoin::bip32::DerivationPath;

        let op0 = fake_outpoint();
        let (wollet, _script0, txout0) = wollet_with_known_input(op0);

        // Get the wallet's real signer fingerprint.
        let wallet_fp = *wollet
            .signers()
            .first()
            .expect("TESTNET_DESC must have at least one signer");

        // The secp256k1 generator G — a valid but unrelated public key.
        let fake_pk: elements::bitcoin::PublicKey =
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                .parse()
                .expect("parse G");

        let ext_txid = "0505050505050505050505050505050505050505050505050505050505050505"
            .parse()
            .expect("txid");
        let op_ext = OutPoint::new(ext_txid, 0);
        let mut ext_input = elements::pset::Input::from_prevout(op_ext);
        // Inject the wallet's fingerprint directly into bip32_derivation.
        ext_input
            .bip32_derivation
            .insert(fake_pk, (wallet_fp, DerivationPath::default()));

        let mut pset = pset_with_input(op0, txout0);
        pset.add_input(ext_input);

        let err = wollet
            .sponsor_prepare(&mut pset, op0, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
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

    // ── utxo_secrets fields ───────────────────────────────────────────────────

    /// Verifies that `utxo_secrets` faithfully returns all blinding-factor fields
    /// and metadata from the cache.  Incorrect values here produce a silent invalid
    /// PSET on the app side, so this is the single most important correctness test
    /// for the FeeSponsor data-export path.
    #[test]
    fn utxo_secrets_fields_correct() {
        use elements::{
            confidential::{AssetBlindingFactor, ValueBlindingFactor},
            encode::Decodable,
            hashes::hex::FromHex,
            TxOutSecrets,
        };

        let op = fake_outpoint();
        let (mut wollet, _script, _txout) = wollet_with_known_input(op);

        // Insert non-zero blinding factors so utxos_inner does not filter this
        // UTXO out via is_explicit() (which skips BF-zero entries).
        let expected_asset = AssetId::default();
        let expected_value = 10_000u64;
        let expected_asset_bf =
            AssetBlindingFactor::from_slice(&[1u8; 32]).expect("valid scalar");
        let expected_value_bf =
            ValueBlindingFactor::from_slice(&[2u8; 32]).expect("valid scalar");
        wollet.cache.extend_unblinded([(
            op,
            TxOutSecrets::new(
                expected_asset,
                expected_asset_bf,
                expected_value,
                expected_value_bf,
            ),
        )]);

        // extend_unblinded adds to cache.unblinded but NOT to cache.unspent.
        // update_unspent only registers an outpoint as unspent when it is already
        // in cache.unblinded at the time update() runs.  Re-trigger update() so
        // that op is now tracked as an unspent output.
        wollet
            .cache
            .update(&[(op.txid, None)], &[], &[], false, vec![])
            .expect("cache re-update after extend_unblinded");

        let secrets = wollet.utxo_secrets(op).expect("utxo_secrets must succeed");

        // txid and vout must round-trip the outpoint.
        assert_eq!(secrets.txid, op.txid);
        assert_eq!(secrets.vout, op.vout);

        // Blinding secrets must match what was inserted in the cache.
        assert_eq!(secrets.asset, expected_asset);
        assert_eq!(secrets.value, expected_value);
        assert_eq!(secrets.asset_bf, expected_asset_bf);
        assert_eq!(secrets.value_bf, expected_value_bf);

        // tx_hex must be parseable as a Transaction.
        let tx_bytes = Vec::<u8>::from_hex(&secrets.tx_hex).expect("tx_hex is valid hex");
        Transaction::consensus_decode(&tx_bytes[..]).expect("tx_hex decodes as Transaction");

        // elwpkh produces segwit outputs.
        assert!(secrets.is_segwit, "elwpkh descriptor must report is_segwit = true");
    }

    /// An unconfirmed UTXO (registered with `height = None`) must report `height = None`
    /// in `UtxoSecrets`.  The app uses this field to decide whether the UTXO is safe to
    /// commit to as a fee sponsor; returning a wrong value could lead to spending an
    /// unconfirmed output whose parent may be replaced.
    #[test]
    fn utxo_secrets_height_none_when_unconfirmed() {
        use elements::{
            confidential::{AssetBlindingFactor, ValueBlindingFactor},
            TxOutSecrets,
        };

        // wollet_with_known_input registers the tx with height = None (unconfirmed).
        let op = fake_outpoint();
        let (mut wollet, _script, _txout) = wollet_with_known_input(op);

        // Seed blinding secrets so the UTXO appears in utxos().
        wollet.cache.extend_unblinded([(
            op,
            TxOutSecrets::new(
                AssetId::default(),
                AssetBlindingFactor::from_slice(&[1u8; 32]).expect("valid scalar"),
                10_000,
                ValueBlindingFactor::from_slice(&[2u8; 32]).expect("valid scalar"),
            ),
        )]);
        wollet
            .cache
            .update(&[(op.txid, None)], &[], &[], false, vec![])
            .expect("re-trigger to track as unspent");

        let secrets = wollet.utxo_secrets(op).expect("utxo_secrets must succeed");

        assert_eq!(
            secrets.height, None,
            "unconfirmed UTXO must report height = None"
        );
    }

    // ── utxo_secrets: blinding-key unblind round-trip ─────────────────────────

    /// Verifies the critical spec property: `TxOut.unblind(secrets.blinding_key)` —
    /// where `blinding_key` comes from `utxo_secrets` — reproduces exactly the same
    /// `asset`, `value`, `asset_bf` and `value_bf` that `utxo_secrets` itself returns.
    ///
    /// This is the exact call path the app uses to build an `ExternalUtxo` via
    /// `lwk_bindings`.  A wrong blinding key produces a silent invalid PSET on the app
    /// side (the transaction broadcasts but the funds are unrecoverable).
    ///
    /// A real confidential output is required here because `TxOut.unblind` only works on
    /// confidential (Pedersen-commitment) outputs.  `pset.blind_last` creates one using
    /// the wallet's slip77-derived blinding public key, so the matching secret key
    /// returned by `utxo_secrets` must be able to reverse it.
    #[test]
    fn utxo_secrets_blinding_key_unblind_round_trip() {
        use elements::{confidential::{AssetBlindingFactor, ValueBlindingFactor}, TxOutSecrets};
        use std::collections::HashMap;

        let op = fake_outpoint();
        let (mut wollet, script, _) = wollet_with_known_input(op);

        // Derive the slip77 blinding key for our wallet script.
        // This is the same key that `utxo_secrets` will return.
        let blinding_sk = wollet
            .descriptor
            .blinding_key_for_script(&script)
            .expect("blinding key derivable from slip77 descriptor");
        let blinding_pk =
            elements::secp256k1_zkp::PublicKey::from_secret_key(&crate::EC, &blinding_sk);

        // Build a minimal PSET: one explicit input + one confidential output to our script.
        let asset = wollet.policy_asset();
        let value = 20_000u64;

        let mut pset = PartiallySignedTransaction::default();

        // Input: use the existing outpoint registered by wollet_with_known_input.
        let mut inp = elements::pset::Input::from_prevout(op);
        inp.witness_utxo = Some(explicit_txout(script.clone()));
        pset.add_input(inp);

        // Output: confidential, to our wallet script.
        // `asset` and `amount` are the PSET metadata fields read by `blind_last`.
        // `blinding_key` is the recipient's slip77-derived public key.
        // `blinder_index = Some(0)` tells `blind_last` to use input[0]'s asset for the
        // surjection proof.
        let out = elements::pset::Output {
            script_pubkey: script.clone(),
            amount: Some(value),
            asset: Some(asset),
            blinding_key: Some(elements::bitcoin::PublicKey::new(blinding_pk)),
            blinder_index: Some(0),
            ..Default::default()
        };
        pset.add_output(out);

        // Provide input's TxOutSecrets so `blind_last` can build the surjection proof.
        // The input is explicit, so its blinding factors are zero.
        let mut inp_txout_sec: HashMap<usize, TxOutSecrets> = HashMap::new();
        inp_txout_sec.insert(
            0,
            TxOutSecrets {
                asset,
                asset_bf: AssetBlindingFactor::zero(),
                value,
                value_bf: ValueBlindingFactor::zero(),
            },
        );

        // Blind the PSET output.
        let mut rng = rand::thread_rng();
        pset.blind_last(&mut rng, &crate::EC, &inp_txout_sec)
            .expect("blind_last must succeed");

        // Extract the blinded transaction.
        let blinded_tx = pset.extract_tx().expect("extract_tx");
        // Immediately unblind output[0] to recover the actual TxOutSecrets (the
        // asset_bf and value_bf were chosen randomly by `blind_last`).
        let blinded_txout = blinded_tx.output[0].clone();
        let setup_secrets = blinded_txout
            .unblind(&crate::EC, blinding_sk)
            .expect("setup unblind must work: blinding_sk must match the blinding_key set above");

        // Register the blinded transaction in the cache.
        let blinded_op = OutPoint::new(blinded_tx.txid(), 0);
        wollet
            .cache
            .update(
                &[(blinded_op.txid, None)],
                &[],
                &[(blinded_op.txid, blinded_tx)],
                false,
                vec![],
            )
            .expect("cache update with blinded tx");
        wollet.cache.extend_unblinded([(blinded_op, setup_secrets)]);
        // Re-trigger so blinded_op is tracked as unspent (see utxo_secrets_fields_correct).
        wollet
            .cache
            .update(&[(blinded_op.txid, None)], &[], &[], false, vec![])
            .expect("cache re-update after extend_unblinded");

        // ── Exercise ──────────────────────────────────────────────────────────
        let result = wollet
            .utxo_secrets(blinded_op)
            .expect("utxo_secrets must succeed");

        // Retrieve the stored (confidential) TxOut.
        let stored_txout = wollet
            .cache
            .tx(&blinded_op.txid)
            .expect("tx must be in cache")
            .output[blinded_op.vout as usize]
            .clone();

        // The spec guarantee: `TxOut.unblind(blinding_key)` must succeed and reproduce
        // the fields returned by `utxo_secrets`.  This is the exact call the app makes.
        let unblinded = stored_txout
            .unblind(&crate::EC, result.blinding_key)
            .expect(
                "TxOut.unblind(secrets.blinding_key) must succeed — this is the app's call path",
            );

        assert_eq!(unblinded.asset, result.asset, "asset must round-trip through unblind");
        assert_eq!(unblinded.value, result.value, "value must round-trip through unblind");
        assert_eq!(
            unblinded.asset_bf, result.asset_bf,
            "asset_bf must round-trip through unblind"
        );
        assert_eq!(
            unblinded.value_bf, result.value_bf,
            "value_bf must round-trip through unblind"
        );
    }

    // ── sponsor_prepare Rule-2 edge cases ─────────────────────────────────────

    /// Two wallet inputs in the PSET → Rule 2 must reject with SponsorInputMismatch.
    ///
    /// Sponsor must co-sign **exactly one** input.  Allowing two would open a
    /// double-spend vector against the sponsor UTXO.
    #[test]
    fn sponsor_prepare_two_wallet_inputs_rejected() {
        let op0 = fake_outpoint();
        let (mut wollet, _script0, txout0) = wollet_with_known_input(op0);

        // Derive a second wallet script (index 1) and register it in paths.
        let desc = wollet.descriptor.clone();
        let (script1, _, _) = wollet
            .cache
            .get_or_derive(Chain::External, ChildNumber::from_normal_idx(1).unwrap(), &desc)
            .expect("derive script1");
        wollet.cache.paths.insert(
            script1.clone(),
            (Chain::External, ChildNumber::from_normal_idx(1).unwrap()),
        );
        let txout1 = explicit_txout(script1);

        // Build PSET with two wallet-owned inputs.
        let op1_txid = "0202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .expect("txid");
        let op1 = OutPoint::new(op1_txid, 0);
        let mut pset = pset_with_input(op0, txout0);
        let mut input1 = elements::pset::Input::from_prevout(op1);
        input1.witness_utxo = Some(txout1);
        pset.add_input(input1);

        let err = wollet
            .sponsor_prepare(&mut pset, op0, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
            "unexpected: {err}"
        );
    }

    /// PSET contains the correct wallet input at `op`, but `sponsor_prepare` is
    /// called with a different outpoint → Rule 2 must reject with SponsorInputMismatch.
    ///
    /// Ensures the sponsor only signs the UTXO it declared it reserved, not any
    /// other wallet UTXO that happens to be in the PSET.
    #[test]
    fn sponsor_prepare_wrong_outpoint_rejected() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        // PSET has the wallet input at `op`; we claim a different outpoint `op2`.
        let mut pset = pset_with_input(op, txout);
        let op2_txid = "0303030303030303030303030303030303030303030303030303030303030303"
            .parse()
            .expect("txid");
        let op2 = OutPoint::new(op2_txid, 1);

        let err = wollet
            .sponsor_prepare(&mut pset, op2, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
            "unexpected: {err}"
        );
    }

    // ── sponsor_prepare happy-path / Alto ─────────────────────────────────────

    /// `SponsorPrepared.txid` must equal `pset.extract_tx().txid()` computed
    /// *before* calling `sponsor_prepare`.
    ///
    /// Liquid segwit txids commit only to the transaction body (inputs + outputs
    /// + locktime), excluding witness data.  `add_details` only populates PSET
    /// metadata (derivation paths, xpub origin) and does not alter the tx body,
    /// so the txid must be identical before and after.
    #[test]
    fn sponsor_prepare_txid_stable_after_add_details() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);
        let mut pset = pset_with_input(op, txout);

        // Record txid before sponsor_prepare touches the PSET.
        let pre_txid = pset.extract_tx().unwrap().txid();

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        assert_eq!(
            prepared.txid, pre_txid,
            "txid must be stable across add_details (segwit excludes witness)"
        );
    }

    /// Deposit output whose PSET `blinding_key` equals the deposit address
    /// blinding pubkey → `blinding_key_matches = Some(true)`.
    #[test]
    fn sponsor_prepare_blinding_key_matches_true() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        let deposit_addr: elements::Address = TESTNET_CONF_ADDR.parse().expect("parse addr");
        let blinding_pubkey = deposit_addr.blinding_pubkey.expect("confidential addr");
        let deposit_spk = deposit_addr.script_pubkey();

        let policy = wollet.policy_asset();
        let mut pset = pset_with_input(op, txout);
        let mut out = Output::new_explicit(deposit_spk, 10_000, policy, None);
        // Set blinding_key to the correct key from deposit_address.
        out.blinding_key = Some(elements::bitcoin::PublicKey::new(blinding_pubkey));
        pset.add_output(out);

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        let deposit_out = prepared
            .outputs
            .iter()
            .find(|o| matches!(o.kind, SponsorOutputKind::Deposit))
            .expect("deposit output must be present");
        assert_eq!(
            deposit_out.blinding_key_matches,
            Some(true),
            "correct blinding key must yield Some(true)"
        );
    }

    /// Deposit output whose PSET `blinding_key` differs from the deposit address
    /// blinding pubkey → `blinding_key_matches = Some(false)`.
    #[test]
    fn sponsor_prepare_blinding_key_matches_false() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        let deposit_addr: elements::Address = TESTNET_CONF_ADDR.parse().expect("parse addr");
        let deposit_spk = deposit_addr.script_pubkey();

        // The secp256k1 generator G — a valid but different public key.
        let different_bk: elements::bitcoin::PublicKey =
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                .parse()
                .expect("parse generator pubkey");

        let policy = wollet.policy_asset();
        let mut pset = pset_with_input(op, txout);
        let mut out = Output::new_explicit(deposit_spk, 10_000, policy, None);
        out.blinding_key = Some(different_bk);
        pset.add_output(out);

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        let deposit_out = prepared
            .outputs
            .iter()
            .find(|o| matches!(o.kind, SponsorOutputKind::Deposit))
            .expect("deposit output must be present");
        assert_eq!(
            deposit_out.blinding_key_matches,
            Some(false),
            "wrong blinding key must yield Some(false)"
        );
    }

    /// Deposit output with no `blinding_key` in the PSET output
    /// → `blinding_key_matches = None`.
    #[test]
    fn sponsor_prepare_blinding_key_matches_none() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        let deposit_addr: elements::Address = TESTNET_CONF_ADDR.parse().expect("parse addr");
        let deposit_spk = deposit_addr.script_pubkey();

        let policy = wollet.policy_asset();
        let mut pset = pset_with_input(op, txout);
        // new_explicit leaves blinding_key = None.
        pset.add_output(Output::new_explicit(deposit_spk, 10_000, policy, None));

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        let deposit_out = prepared
            .outputs
            .iter()
            .find(|o| matches!(o.kind, SponsorOutputKind::Deposit))
            .expect("deposit output must be present");
        assert_eq!(
            deposit_out.blinding_key_matches, None,
            "absent blinding_key must yield None"
        );
    }

    // ── Médio: G4 — blinding_key_matches = None for non-Deposit outputs ─────────

    /// Fee, Mine and Other outputs must always have `blinding_key_matches = None`,
    /// even when a `blinding_key` field is explicitly set in the PSET output.
    /// Only Deposit outputs may carry a non-None value.
    /// [G4] This asserts the invariant that was previously only implied by the
    /// `_ => None` arm in the classification code.
    #[test]
    fn sponsor_output_blinding_key_matches_null_for_non_deposit() {
        let op = fake_outpoint();
        let (mut wollet, _script, txout) = wollet_with_known_input(op);

        // Derive a Mine script.
        let desc = wollet.descriptor.clone();
        let (mine_script, _, _) = wollet
            .cache
            .get_or_derive(Chain::External, ChildNumber::from_normal_idx(1).unwrap(), &desc)
            .expect("derive mine script");
        wollet.cache.paths.insert(
            mine_script.clone(),
            (Chain::External, ChildNumber::from_normal_idx(1).unwrap()),
        );

        let other_script = Script::from(vec![0x51]); // OP_1
        let policy = wollet.policy_asset();

        // A valid blinding key (secp256k1 generator G) — set it on Mine and Other to
        // confirm the classification code ignores it for non-Deposit kinds.
        let bk: elements::bitcoin::PublicKey =
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
                .parse()
                .expect("parse generator pubkey");

        let mut pset = pset_with_input(op, txout);

        // Fee output (empty script) — blinding_key intentionally absent.
        pset.add_output(Output::new_explicit(Script::default(), 300, policy, None));

        // Mine output with blinding_key explicitly set.
        let mut mine_out = Output::new_explicit(mine_script, 5_000, policy, None);
        mine_out.blinding_key = Some(bk);
        pset.add_output(mine_out);

        // Other output with blinding_key explicitly set.
        let mut other_out = Output::new_explicit(other_script, 3_000, policy, None);
        other_out.blinding_key = Some(bk);
        pset.add_output(other_out);

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        for out in &prepared.outputs {
            assert_eq!(
                out.blinding_key_matches, None,
                "output[{}] ({:?}) must have blinding_key_matches = None (non-deposit)",
                out.index, out.kind
            );
        }
    }

    // ── Médio: rule-order guard (Rule 3 before Rule 4) ───────────────────────

    /// When both Rule 3 (bad sighash) and Rule 4 (absent witness_utxo) are triggered
    /// on the same input, Rule 3 must fire first.
    /// This confirms that witness_utxo absence is never masked by a prior rule.
    #[test]
    fn sponsor_prepare_witness_utxo_absent_rule_order() {
        let op = fake_outpoint();
        let (wollet, _script, _txout) = wollet_with_known_input(op);

        // Input has sighash NONE (Rule 3 trigger) AND no witness_utxo (Rule 4 trigger).
        let mut pset = PartiallySignedTransaction::default();
        let mut input = elements::pset::Input::from_prevout(op); // witness_utxo = None
        input.sighash_type =
            Some(PsbtSighashType::from_u32(EcdsaSighashType::None.as_u32()));
        pset.add_input(input);

        let err = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .unwrap_err();

        // Rule 3 (sighash) is evaluated before Rule 4 (witness_utxo), so this must
        // return SPONSOR_SIGHASH_NOT_ALL — not SPONSOR_WITNESS_UTXO_MISMATCH.
        assert!(
            err.to_string().starts_with("SPONSOR_SIGHASH_NOT_ALL"),
            "Rule 3 must precede Rule 4; expected SPONSOR_SIGHASH_NOT_ALL, got: {err}"
        );
    }

    // ── Baixo: multiple fee outputs ───────────────────────────────────────────

    /// A PSET with two fee outputs (empty script_pubkey, explicit values) must report
    /// `SponsorPrepared.fee` equal to the sum of both values.
    #[test]
    fn sponsor_prepare_multiple_fee_outputs() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        let policy = wollet.policy_asset();
        let mut pset = pset_with_input(op, txout);
        pset.add_output(Output::new_explicit(Script::default(), 200, policy, None));
        pset.add_output(Output::new_explicit(Script::default(), 300, policy, None));

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        assert_eq!(
            prepared.fee, 500,
            "fee must be the sum of all fee outputs (200 + 300 = 500)"
        );
    }

    /// PSET with no fee output (no empty-script output) → `SponsorPrepared.fee == 0`.
    #[test]
    fn sponsor_prepare_fee_zero_no_fee_output() {
        let op = fake_outpoint();
        let (wollet, _script, txout) = wollet_with_known_input(op);

        let deposit_addr: elements::Address = TESTNET_CONF_ADDR.parse().expect("parse addr");
        let deposit_spk = deposit_addr.script_pubkey();

        let policy = wollet.policy_asset();
        let mut pset = pset_with_input(op, txout);
        // Only a deposit output — no empty-script fee output.
        pset.add_output(Output::new_explicit(deposit_spk, 10_000, policy, None));

        let prepared = wollet
            .sponsor_prepare(&mut pset, op, TESTNET_CONF_ADDR)
            .expect("sponsor_prepare must succeed");

        assert_eq!(prepared.fee, 0, "PSET without fee output must report fee = 0");
    }

    // ── Médio: Rule 2 — spent UTXO still detected by outpoint ────────────────

    /// An extra PSET input pointing to a wallet UTXO that was once live (confirmed and
    /// unblinded) but has since been spent by another transaction.
    ///
    /// Rule 2 uses `cache.tx()` lookup (not `wollet.utxos()`), so the spent outpoint is
    /// still recognised as a wallet input → `wallet_inputs.len() == 2` →
    /// `SPONSOR_INPUT_MISMATCH`.
    ///
    /// This distinguishes the outpoint-based detection from a hypothetical `utxos()`-based
    /// check that would silently miss the spent input.
    #[test]
    fn sponsor_extra_spent_wallet_input_rejected() {
        use elements::{
            confidential::{AssetBlindingFactor, ValueBlindingFactor},
            AssetIssuance, Sequence, TxInWitness, TxOutSecrets,
        };

        let op0 = fake_outpoint();
        let (mut wollet, _script0, txout0) = wollet_with_known_input(op0);

        // ── Register op1 as a second wallet-owned UTXO ───────────────────────
        let desc = wollet.descriptor.clone();
        let (script1, _, _) = wollet
            .cache
            .get_or_derive(Chain::External, ChildNumber::from_normal_idx(1).unwrap(), &desc)
            .expect("derive script1");
        wollet.cache.paths.insert(
            script1.clone(),
            (Chain::External, ChildNumber::from_normal_idx(1).unwrap()),
        );
        let txout1 = explicit_txout(script1);
        let op1_txid = "0303030303030303030303030303030303030303030303030303030303030303"
            .parse()
            .expect("op1 txid");
        let op1 = OutPoint::new(op1_txid, 0);
        let tx1 = Transaction {
            version: 2,
            lock_time: LockTime::ZERO,
            input: vec![],
            output: vec![txout1],
        };
        wollet
            .cache
            .update(&[(op1.txid, None)], &[], &[(op1.txid, tx1)], false, vec![])
            .expect("cache update for op1 tx");

        // Seed blinding factors so op1 enters cache.unspent.
        wollet.cache.extend_unblinded([(
            op1,
            TxOutSecrets::new(
                AssetId::default(),
                AssetBlindingFactor::from_slice(&[3u8; 32]).expect("bf"),
                5_000,
                ValueBlindingFactor::from_slice(&[4u8; 32]).expect("bf"),
            ),
        )]);
        // Re-trigger so op1 is added to cache.unspent (extend_unblinded alone does not).
        wollet
            .cache
            .update(&[(op1.txid, None)], &[], &[], false, vec![])
            .expect("re-trigger to track op1 as unspent");
        let utxos_before: Vec<_> = wollet
            .utxos()
            .expect("utxos")
            .into_iter()
            .filter(|u| u.outpoint == op1)
            .collect();
        assert!(!utxos_before.is_empty(), "op1 must be unspent before spending");

        // ── Spend op1 by registering a tx that uses it as input ──────────────
        let spending_txid = "0404040404040404040404040404040404040404040404040404040404040404"
            .parse()
            .expect("spending txid");
        let spending_tx = Transaction {
            version: 2,
            lock_time: LockTime::ZERO,
            input: vec![elements::TxIn {
                previous_output: op1,
                is_pegin: false,
                script_sig: Script::new(),
                sequence: Sequence::MAX,
                asset_issuance: AssetIssuance::default(),
                witness: TxInWitness::default(),
            }],
            output: vec![],
        };
        wollet
            .cache
            .update(
                &[(spending_txid, Some(1))],
                &[],
                &[(spending_txid, spending_tx)],
                false,
                vec![],
            )
            .expect("cache update with spending tx");

        // op1 must no longer appear in utxos() — it was spent.
        let utxos_after: Vec<_> = wollet
            .utxos()
            .expect("utxos")
            .into_iter()
            .filter(|u| u.outpoint == op1)
            .collect();
        assert!(
            utxos_after.is_empty(),
            "op1 must not appear in utxos() after spending"
        );

        // ── PSET: op0 (reserved, with witness_utxo) + op1 (spent, no witness_utxo) ──
        let mut pset = pset_with_input(op0, txout0);
        let extra_input = elements::pset::Input::from_prevout(op1); // no witness_utxo
        pset.add_input(extra_input);

        // Rule 2: cache.tx(op1.txid) still returns op1's tx → op1 detected as wallet input.
        // wallet_inputs == {op0, op1} ≠ {op0} → SPONSOR_INPUT_MISMATCH.
        let err = wollet
            .sponsor_prepare(&mut pset, op0, TESTNET_CONF_ADDR)
            .unwrap_err();
        assert!(
            err.to_string().starts_with("SPONSOR_INPUT_MISMATCH"),
            "spent wallet UTXO in PSET must yield SPONSOR_INPUT_MISMATCH, got: {err}"
        );
    }
}
