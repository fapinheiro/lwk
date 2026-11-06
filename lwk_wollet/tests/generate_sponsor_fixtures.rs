// ── Golden Fixture Generator for FeeSponsor v1 ─────────────────────────────
//
// Generates JSON fixtures from a real regtest sponsor flow.
// These fixtures are shared across 3 repos (LWK, Backend, iOS) to detect
// cross-repo regressions.
//
// Run: cargo test --test e2e -- test_generate_sponsor_fixtures --ignored --nocapture
//
// Output: lwk_wollet/tests/data/sponsor_fixtures/
//
// After generating, copy fixtures to:
//   tappix-api/src/test/resources/fixtures/sponsor/
//   tappix-ios/TappixAppTests/Fixtures/sponsor/

use crate::test_wollet::*;
use elements::encode::deserialize;
use elements::hex::FromHex;
use elements::{OutPoint, Transaction};
use lwk_common::Signer;
use lwk_signer::*;
use lwk_test_util::*;
use lwk_wollet::*;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("sponsor_fixtures");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn errors_dir() -> PathBuf {
    let dir = fixtures_dir().join("errors");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_json(dir: &PathBuf, name: &str, value: &Value) {
    let path = dir.join(name);
    let content = serde_json::to_string_pretty(value).unwrap();
    fs::write(&path, content).unwrap();
    println!("  wrote {}", path.display());
}

fn write_text(dir: &PathBuf, name: &str, content: &str) {
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    println!("  wrote {}", path.display());
}

/// Generates sponsor golden fixtures from a real regtest flow.
///
/// This test is #[ignore] because it requires a running elementsd + electrum
/// (started by TestEnvBuilder). Run it explicitly when regenerating fixtures.
#[test]
#[ignore]
fn test_generate_sponsor_fixtures() {
    let dir = fixtures_dir();
    let errors = errors_dir();
    println!("Generating sponsor fixtures in: {}", dir.display());

    let env = TestEnvBuilder::from_env().with_electrum().build();

    // ── Setup wallets ───────────────────────────────────────────────────────

    // Wallet A = sponsor (has L-BTC)
    let signer_a = generate_signer();
    let view_key_a = generate_view_key();
    let desc_a = format!("ct({view_key_a},elwpkh({}/*))", signer_a.xpub());
    let mut wallet_a = TestWollet::new(test_client_electrum(&env.electrum_url()), &desc_a);

    // Wallet B = user (has issued asset, no L-BTC)
    let signer_b = generate_signer();
    let view_key_b = generate_view_key();
    let desc_b = format!("ct({view_key_b},elwpkh({}/*))", signer_b.xpub());
    let mut wallet_b = TestWollet::new(test_client_electrum(&env.electrum_url()), &desc_b);

    // Wallet C = deposit destination (partner)
    let view_key_c = generate_view_key();
    let desc_c = format!(
        "ct({view_key_c},elwpkh({}/*))",
        generate_signer().xpub()
    );
    let wallet_c = TestWollet::new(test_client_electrum(&env.electrum_url()), &desc_c);
    let addr_c = wallet_c.address();

    let lbtc = wallet_a.policy_asset();

    // Fund A with L-BTC
    wallet_a.fund_btc(&env);
    wallet_a.sync();

    // Issue asset and send to B
    let signers_a = [&AnySigner::Software(signer_a.clone())];
    let (asset, _token) = wallet_a.issueasset(&signers_a, 1_000, 0, None, None);
    let txid = wallet_a.send_asset(&signers_a, &wallet_b.address(), &asset, None);
    wait_for_tx(&mut wallet_b.wollet, &mut wallet_b.client, &txid);
    wallet_b.sync();

    assert_eq!(wallet_b.balance(&asset), 10);
    assert_eq!(wallet_b.balance(&lbtc), 0);

    // ── Step 1: wallet_utxos (pick sponsor UTXO) ────────────────────────────

    let sponsor_utxo = wallet_a
        .wollet
        .utxos()
        .unwrap()
        .into_iter()
        .find(|u| u.unblinded.asset == lbtc)
        .unwrap();
    let sponsor_outpoint = sponsor_utxo.outpoint;

    // ── Step 2: wallet_utxo_secrets ─────────────────────────────────────────

    let secrets = wallet_a.wollet.utxo_secrets(sponsor_outpoint).unwrap();

    let utxo_secrets_json = json!({
        "txid": secrets.txid.to_string(),
        "vout": secrets.vout,
        "tx_hex": secrets.tx_hex,
        "script_pubkey": secrets.script_pubkey,
        "asset": secrets.asset.to_string(),
        "asset_bf": secrets.asset_bf.to_string(),
        "value": secrets.value,
        "value_bf": secrets.value_bf.to_string(),
        "blinding_key": secrets.blinding_key,
        "height": secrets.height,
        "max_weight_to_satisfy": secrets.max_weight_to_satisfy,
        "is_segwit": secrets.is_segwit
    });
    write_json(&dir, "utxo-secrets.json", &utxo_secrets_json);

    // ── Step 3: Build PSET (as the iOS app would) ───────────────────────────

    let external_utxo = external_utxo_from_secrets(secrets);
    let addr_a = wallet_a.address();

    let mut pset = wallet_b
        .tx_builder()
        .add_recipient(&addr_c, 5, asset)
        .unwrap()
        .add_external_utxos(vec![external_utxo])
        .unwrap()
        .drain_lbtc_wallet()
        .drain_lbtc_to(addr_a)
        .finish()
        .unwrap();

    let pset_unsigned_base64 = pset.to_string();
    write_text(&dir, "pset-unsigned.base64", &pset_unsigned_base64);

    let txid_before = pset.extract_tx().unwrap().txid();

    // ── Step 4: wallet_sponsor_prepare ──────────────────────────────────────

    let prepared = wallet_a
        .wollet
        .sponsor_prepare(&mut pset, sponsor_outpoint, &addr_c.to_string())
        .unwrap();

    assert_eq!(prepared.txid, txid_before);
    assert!(prepared.fee > 0);

    let outputs_json: Vec<Value> = prepared
        .outputs
        .iter()
        .map(|o| {
            json!({
                "index": o.index,
                "script_pubkey": o.script_pubkey,
                "kind": o.kind,
                "blinding_key_matches": o.blinding_key_matches
            })
        })
        .collect();

    let prepared_json = json!({
        "pset": pset.to_string(),
        "txid": prepared.txid.to_string(),
        "fee": prepared.fee,
        "outputs": outputs_json
    });
    write_json(&dir, "pset-prepared.json", &prepared_json);
    write_json(&dir, "outputs.json", &json!(outputs_json));

    // ── Step 5: signer_sign (sponsor co-signs) ──────────────────────────────

    let sigs_a = signer_a.sign(&mut pset).unwrap();
    assert!(sigs_a > 0);

    let pset_signed_base64 = pset.to_string();
    write_text(&dir, "pset-signed.base64", &pset_signed_base64);

    // ── Step 6: wallet_pset_details (after signing) ─────────────────────────

    let details = wallet_a.wollet.get_details(&pset).unwrap();

    // Convert balance (BTreeMap<AssetId, i64>) to HashMap<String, i64>
    let balance_map: serde_json::Map<String, Value> = details
        .balance
        .balances
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();

    // Convert fees (HashMap<AssetId, u64>) to HashMap<String, u64>
    let fees_map: serde_json::Map<String, Value> = details
        .balance
        .fees
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();

    // Map fingerprints to SignerShortDetails format (name + fingerprint)
    // The sponsor signer is signer_a; the user signer is signer_b
    let sponsor_fp = signer_a.fingerprint().to_string();
    let user_fp = signer_b.fingerprint().to_string();

    let has_sigs: Vec<Value> = details
        .fingerprints_has()
        .iter()
        .map(|fp| {
            let name = if fp.to_string() == sponsor_fp {
                "sponsor"
            } else if fp.to_string() == user_fp {
                "user"
            } else {
                "unknown"
            };
            json!({ "name": name, "fingerprint": fp.to_string() })
        })
        .collect();

    let missing_sigs: Vec<Value> = details
        .fingerprints_missing()
        .iter()
        .map(|fp| {
            let name = if fp.to_string() == sponsor_fp {
                "sponsor"
            } else if fp.to_string() == user_fp {
                "user"
            } else {
                "unknown"
            };
            json!({ "name": name, "fingerprint": fp.to_string() })
        })
        .collect();

    let pset_details_json = json!({
        "txid": prepared.txid.to_string(),
        "has_signatures_from": has_sigs,
        "missing_signatures_from": missing_sigs,
        "balance": balance_map,
        "fees": fees_map,
        "issuances": [],
        "reissuances": [],
        "warnings": ""
    });
    write_json(&dir, "pset-details.json", &pset_details_json);

    // ── Step 7: metadata ────────────────────────────────────────────────────

    let generated_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let lwk_commit = option_env!("LWK_GIT_HASH").unwrap_or("unknown");
    let metadata = json!({
        "version": "v1",
        "lwk_version": env!("CARGO_PKG_VERSION"),
        "lwk_commit": lwk_commit,
        "network": "regtest",
        "generated_at": generated_at,
        "notes": "Generated by test_generate_sponsor_fixtures in lwk_wollet/tests"
    });
    write_json(&dir, "metadata.json", &metadata);

    // ── Error fixtures ──────────────────────────────────────────────────────

    // sighash_not_all: modify sighash on sponsor input
    {
        use elements::pset::PsbtSighashType;

        // Rebuild a fresh PSET for the error case
        let secrets2 = wallet_a.wollet.utxo_secrets(sponsor_outpoint).unwrap();
        let ext2 = external_utxo_from_secrets(secrets2);

        let mut pset_bad = wallet_b
            .tx_builder()
            .add_recipient(&addr_c, 5, asset)
            .unwrap()
            .add_external_utxos(vec![ext2])
            .unwrap()
            .drain_lbtc_wallet()
            .drain_lbtc_to(wallet_a.address())
            .finish()
            .unwrap();

        let idx = pset_bad
            .inputs()
            .iter()
            .position(|inp| {
                OutPoint::new(inp.previous_txid, inp.previous_output_index) == sponsor_outpoint
            })
            .unwrap();

        pset_bad.inputs_mut()[idx].sighash_type =
            Some(PsbtSighashType::from_u32(elements::EcdsaSighashType::None.as_u32()));

        let err = wallet_a
            .wollet
            .sponsor_prepare(&mut pset_bad, sponsor_outpoint, &addr_c.to_string())
            .unwrap_err();

        write_json(
            &errors,
            "sponsor-sighash-not-all.json",
            &json!({
                "error": {
                    "code": -32603,
                    "message": err.to_string()
                }
            }),
        );
    }

    println!("\nFixtures generated successfully in: {}", dir.display());
    println!("Copy to:");
    println!("  tappix-api/src/test/resources/fixtures/sponsor/");
    println!("  tappix-ios/TappixAppTests/Fixtures/sponsor/");
}

/// Helper: convert UtxoSecrets into ExternalUtxo (same as in e2e.rs)
fn external_utxo_from_secrets(secrets: lwk_wollet::sponsor::UtxoSecrets) -> ExternalUtxo {
    let raw: Vec<u8> = Vec::from_hex(&secrets.tx_hex).unwrap();
    let tx: Transaction = deserialize(&raw).unwrap();
    let txout = tx.output[secrets.vout as usize].clone();
    let tx_opt = if secrets.is_segwit { None } else { Some(tx) };
    ExternalUtxo {
        outpoint: OutPoint::new(secrets.txid, secrets.vout),
        txout,
        tx: tx_opt,
        unblinded: elements::TxOutSecrets {
            asset: secrets.asset,
            asset_bf: secrets.asset_bf,
            value: secrets.value,
            value_bf: secrets.value_bf,
        },
        max_weight_to_satisfy: secrets.max_weight_to_satisfy,
    }
}
