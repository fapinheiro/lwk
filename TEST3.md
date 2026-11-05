# FeeSponsor v1 — Testes gerados a partir da spec (TODO3.md)

Fonte de verdade: **TODO3.md**, seções 3.2, 3.3, 3.4 e 4.2.
Implementação de referência: `lwk_wollet/src/sponsor.rs`.

---

## Divergências encontradas (Passo 3)

Nenhuma divergência de lógica entre spec e código. Os gaps encontrados são de **cobertura de teste**:

| Gap | Descrição |
|-----|-----------|
| G1 | `SIGHASH_ALL\|ANYONECANPAY` não estava no array de sighashes inválidos do teste `sponsor_sighash_not_all` |
| G2 | `witness_utxo` **ausente** (vs errado) não coberto para `SPONSOR_WITNESS_UTXO_MISMATCH` |
| G3 | `height = None` para UTXO não confirmado não coberto em `utxo_secrets` |
| G4 | `blinding_key_matches = None` para saídas não-deposit não assertado explicitamente |

---

## Candidatos listados (Passo 4)

### Crítico

- **`utxo_secrets_blinding_key_unblind_round_trip`** — `TxOut.unblind(blinding_key)` reconstrói exatamente `asset`/`value`/`asset_bf`/`value_bf`. Impacto financeiro direto: BF errado = PSET inválido, saque falha silenciosamente. Status: `utxo_secrets_fields_correct` existia mas sem a etapa de unblind real.
- **`sponsor_sighash_all_anyonecanpay_rejected`** [G1] — `SIGHASH_ALL|ANYONECANPAY` deve retornar `SPONSOR_SIGHASH_NOT_ALL`. Sem isso, atacante poderia redirecionar o L-BTC do sponsor.

### Alto

- **`sponsor_witness_utxo_absent_rejected`** [G2] — input correto mas sem `witness_utxo` → `SPONSOR_WITNESS_UTXO_MISMATCH`. Regra 4 tem dois triggers distintos (ausente vs divergente); ausente é a condição mais provável em PSETs mal formados.
- **`utxo_secrets_height_none_when_unconfirmed`** [G3] — UTXO não confirmado deve devolver `height = None`. O app usa `height` para decidir se o UTXO é seguro para usar.
- **`integration_sponsor_full_flow`** [spec 4.2 items 1-4] — wallet A (sponsor L-BTC) + wallet B (asset sem L-BTC) → `utxo_secrets` → app monta PSET → `sponsor_prepare` → `signer_sign` A → B assina → broadcast → confirma.
- **`integration_sponsor_sighash_none_rejected`** [spec 4.2 negativo] — mesmo fluxo mas com sighash `NONE` no input de A → `sponsor_prepare` retorna `SPONSOR_SIGHASH_NOT_ALL`.

### Médio

- **`sponsor_output_blinding_key_matches_null_for_non_deposit`** [G4] — Fee, Mine e Other devem ter `blinding_key_matches = None` explicitamente assertado.
- **`sponsor_prepare_witness_utxo_absent_rule_order`** — garante que Rule 4 (witness_utxo ausente) não é mascarada por Rule 3 se sighash também estiver errado.
- **`utxo_secrets_fields_complete_unblind_step`** — complementa `utxo_secrets_fields_correct` com a chamada real `TxOut.unblind(blinding_key)`.

### Baixo

- **`integration_pset_compat_bindings_0_19`** [spec 4.2 compat] — PSET montado com `lwk_bindings` 0.19.0, incluindo `feeRate` customizado, aceito por `sponsor_prepare` e assinado por `signer_sign` do fork 0.18. Obrigatório antes do release.
- **`sponsor_prepare_multiple_fee_outputs`** — PSET com dois outputs de fee → `fee` = soma dos dois.

---

## Testes implementados (Passo 5)

### Crítico 1 — `utxo_secrets_blinding_key_unblind_round_trip`

**Arquivo:** [lwk_wollet/src/sponsor.rs](lwk_wollet/src/sponsor.rs), bloco `#[cfg(test)]`

Estratégia: cria um output confidencial real via `pset.blind_last` usando a chave de blinding slip77 derivada do descriptor da wallet, armazena no cache, chama `utxo_secrets` e verifica que `TxOut.unblind(secrets.blinding_key)` retorna exatamente os mesmos `asset`/`value`/`asset_bf`/`value_bf`.

```
test sponsor::tests::utxo_secrets_blinding_key_unblind_round_trip ... ok
```

### Crítico 2 — `SIGHASH_ALL|ANYONECANPAY` rejeitado [G1]

**Arquivo:** [lwk_wollet/src/sponsor.rs](lwk_wollet/src/sponsor.rs), teste `sponsor_sighash_not_all`

Estratégia: adicionado `EcdsaSighashType::AllPlusAnyoneCanPay` ao array `bad_sighashes` do teste existente (já era table-driven).

```
test sponsor::tests::sponsor_sighash_not_all ... ok
```

### Médio 1 — `sponsor_output_blinding_key_matches_null_for_non_deposit` [G4]

**Arquivo:** [lwk_wollet/src/sponsor.rs](lwk_wollet/src/sponsor.rs), bloco `#[cfg(test)]`

Estratégia: PSET com outputs Fee, Mine (blinding_key setado) e Other (blinding_key setado) — sem nenhum Deposit. Asserta que todos retornam `blinding_key_matches = None`, confirmando que o campo é ignorado para kinds não-Deposit.

```
test sponsor::tests::sponsor_output_blinding_key_matches_null_for_non_deposit ... ok
```

### Médio 2 — `sponsor_prepare_witness_utxo_absent_rule_order`

**Arquivo:** [lwk_wollet/src/sponsor.rs](lwk_wollet/src/sponsor.rs), bloco `#[cfg(test)]`

Estratégia: input com sighash `NONE` (Rule 3) e `witness_utxo` ausente (Rule 4) simultaneamente. Asserta que o erro é `SPONSOR_SIGHASH_NOT_ALL`, confirmando que Rule 3 é avaliada antes de Rule 4.

```
test sponsor::tests::sponsor_prepare_witness_utxo_absent_rule_order ... ok
```

### Baixo — `sponsor_prepare_multiple_fee_outputs`

**Arquivo:** [lwk_wollet/src/sponsor.rs](lwk_wollet/src/sponsor.rs), bloco `#[cfg(test)]`

Estratégia: PSET com dois outputs de fee (200 + 300 sat). Asserta `SponsorPrepared.fee == 500`.

```
test sponsor::tests::sponsor_prepare_multiple_fee_outputs ... ok
```

---

## Resultado final

```
running 25 tests
test sponsor::tests::sponsor_extra_wallet_input_no_witness_utxo ... ok
test sponsor::tests::sponsor_issuance_not_allowed ... ok
test sponsor::tests::sponsor_output_blinding_key_matches_null_for_non_deposit ... ok
test sponsor::tests::sponsor_output_classification ... ok
test sponsor::tests::sponsor_prepare_blinding_key_matches_none ... ok
test sponsor::tests::sponsor_prepare_blinding_key_matches_false ... ok
test sponsor::tests::sponsor_prepare_deposit_address_invalid_unconfidential ... ok
test sponsor::tests::sponsor_extra_input_fake_wallet_script ... ok
test sponsor::tests::sponsor_extra_input_bip32_derivation_injected ... ok
test sponsor::tests::sponsor_prepare_deposit_address_invalid_wrong_network ... ok
test sponsor::tests::sponsor_prepare_blinding_key_matches_true ... ok
test sponsor::tests::sponsor_prepare_multiple_fee_outputs ... ok
test sponsor::tests::sponsor_prepare_fee_zero_no_fee_output ... ok
test sponsor::tests::sponsor_prepare_input_mismatch_empty_pset ... ok
test sponsor::tests::sponsor_prepare_witness_utxo_absent_rule_order ... ok
test sponsor::tests::sponsor_prepare_wrong_outpoint_rejected ... ok
test sponsor::tests::sponsor_prepare_txid_stable_after_add_details ... ok
test sponsor::tests::sponsor_witness_utxo_absent_rejected ... ok
test sponsor::tests::sponsor_witness_utxo_mismatch ... ok
test sponsor::tests::sponsor_sighash_not_all ... ok
test sponsor::tests::utxo_secrets_fields_correct ... ok
test sponsor::tests::sponsor_prepare_two_wallet_inputs_rejected ... ok
test sponsor::tests::utxo_secrets_not_found ... ok
test sponsor::tests::utxo_secrets_height_none_when_unconfirmed ... ok
test sponsor::tests::utxo_secrets_blinding_key_unblind_round_trip ... ok

test result: ok. 25 passed; 0 failed; 0 ignored
```

---

## Gaps ainda abertos

| Gap | Teste pendente | Prioridade | Observação |
|-----|----------------|------------|------------|
| — | `integration_sponsor_full_flow` | Alto | requer regtest |
| — | `integration_sponsor_sighash_none_rejected` | Alto | requer regtest |
| — | `integration_pset_compat_bindings_0_19` | Baixo | requer lwk_bindings 0.19 — obrigatório antes do release |
