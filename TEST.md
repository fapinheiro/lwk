# TEST.md — Candidatos a testes: FeeSponsor v1 + Response size cap

Gerado a partir dos commits `f38cb7faa` (FeeSponsor v1) e `bbb8863f8` (response size cap).
A spec é a fonte de verdade; o código é consultado apenas para mecânica técnica.

---

## Contexto

### Implementações cobertas

| Commit | Descrição | Arquivo principal |
|--------|-----------|-------------------|
| `f38cb7faa` | FeeSponsor v1: `utxo_secrets` + `sponsor_prepare` | `lwk_wollet/src/sponsor.rs` |
| `bbb8863f8` | Limite de resposta HTTP + cap de error body | `lwk_wollet/src/clients/asyncr/esplora.rs` |

### Testes já existentes (não duplicar)

**sponsor.rs `#[cfg(test)]`:**
- `utxo_secrets_not_found`
- `sponsor_prepare_deposit_address_invalid_wrong_network`
- `sponsor_prepare_deposit_address_invalid_unconfidential`
- `sponsor_prepare_input_mismatch_empty_pset`
- `sponsor_sighash_not_all`
- `sponsor_witness_utxo_mismatch`
- `sponsor_issuance_not_allowed`
- `sponsor_output_classification`

**lwk_wollet/tests/e2e.rs:**
- `test_sponsor_fee_flow`
- `test_sponsor_sighash_none_rejected`

**esplora.rs `#[cfg(test)]`:**
- `get_with_retry_passes_422_through`
- `get_with_retry_401_exhausts_retries`

### Estrutura de suítes

Nenhum módulo novo precisa ser criado:
- Testes unitários de sponsor → `lwk_wollet/src/sponsor.rs` `#[cfg(test)] mod tests` (helpers `wollet_with_known_input`, `pset_with_input` já disponíveis)
- Testes de response-size → `lwk_wollet/src/clients/asyncr/esplora.rs` `#[cfg(test)] mod tests` (helper `serve()` já disponível)

---

## Observação sobre edge case (ordering de erros)

**Input sem `witness_utxo` no sponsor input:**
A Regra 2 de `sponsor_prepare` filtra inputs por `witness_utxo.script_pubkey`.
Se o sponsor input está no PSET mas **sem** `witness_utxo`, ele não entra em `wallet_inputs`
e o erro retornado é `SponsorInputMismatch` (Regra 2) — não `SponsorWitnessUtxoMismatch` (Regra 4).
Comportamento defensivamente correto (rejeita o PSET), mas o erro pode ser confuso operacionalmente.
O teste Médio abaixo documenta esse comportamento; se quiser alterar o erro retornado, ajuste antes de implementar.

---

## FeeSponsor v1 — candidatos a testes

### Critico

- [x] **`utxo_secrets_fields_correct`** — `sponsor.rs` unit
  UTXO real inserido no cache; verificar que `utxo_secrets` retorna `txid`, `vout`, `asset`,
  `value`, `asset_bf`, `value_bf`, `tx_hex` parseável como `Transaction`, e `is_segwit = true`
  para descriptor `elwpkh`.
  _Único teste que valida que os dados exportados ao app são fidedignos. Dados errados produzem PSET inválido silenciosamente._

- [x] **`sponsor_prepare_two_wallet_inputs_rejected`** — `sponsor.rs` unit
  PSET com dois inputs cujo `witness_utxo.script_pubkey` pertence à wallet (derivar dois scripts,
  inserir dois inputs) → `SponsorInputMismatch`.
  _Regra central de segurança: sponsor deve assinar exatamente um input. Falha aqui = double-spend possível._

- [x] **`sponsor_prepare_wrong_outpoint_rejected`** — `sponsor.rs` unit
  PSET com um input da wallet usando script correto, mas o `outpoint` passado a `sponsor_prepare`
  é diferente do input presente → `SponsorInputMismatch`.
  _Garante que o sponsor só assina o UTXO que declarou reservar, não qualquer outro da wallet._

### Alto

- [ ] **`sponsor_prepare_txid_stable_after_add_details`** — `sponsor.rs` unit
  Calcular `pset.extract_tx().txid()` antes de chamar `sponsor_prepare`; verificar que
  `prepared.txid` é idêntico.
  _Propriedade documentada explicitamente: txid é estável porque segwit exclui witness do txid._

- [ ] **`sponsor_prepare_blinding_key_matches_true`** — `sponsor.rs` unit
  Output Deposit com `pset_out.blinding_key` igual ao `blinding_pubkey` do `deposit_address`
  → `blinding_key_matches = Some(true)`.
  _Permite ao app verificar que o usuário construiu o output corretamente._

- [ ] **`sponsor_prepare_blinding_key_matches_false`** — `sponsor.rs` unit
  Output Deposit com `pset_out.blinding_key` diferente do `blinding_pubkey` do `deposit_address`
  → `blinding_key_matches = Some(false)`.
  _Detecta adulteração: app sinaliza que o endereço de depósito foi alterado._

- [ ] **`sponsor_prepare_blinding_key_matches_none`** — `sponsor.rs` unit
  Output Deposit sem `blinding_key` no PSET output (campo ausente)
  → `blinding_key_matches = None`.
  _Comportamento documentado na spec; output construído sem blinding_key (raro, mas possível)._

- [ ] **`sponsor_prepare_fee_zero_no_fee_output`** — `sponsor.rs` unit
  PSET sem nenhum output com `script_pubkey` vazio → `prepared.fee == 0`.
  _Verifica o cálculo de fee; PSET tecnicamente válido sem fee output deve retornar zero._

### Medio

- [ ] **`sponsor_prepare_input_without_witness_utxo`** — `sponsor.rs` unit
  PSET com um input cujo `previous_txid`/`previous_output_index` bate com `outpoint`, mas
  `witness_utxo = None` → `SponsorInputMismatch` (não `SponsorWitnessUtxoMismatch`).
  _Documenta o ordering de erros: Regra 2 é verificada antes da Regra 4._

- [ ] **`sponsor_issuance_inflation_keys_rejected`** — `sponsor.rs` unit
  Input com `issuance_inflation_keys = Some(...)` → `SponsorIssuanceNotAllowed`.

- [ ] **`sponsor_issuance_entropy_rejected`** — `sponsor.rs` unit
  Input com `issuance_asset_entropy = Some(...)` → `SponsorIssuanceNotAllowed`.

- [ ] **`sponsor_issuance_nonce_rejected`** — `sponsor.rs` unit
  Input com `issuance_blinding_nonce = Some(...)` → `SponsorIssuanceNotAllowed`.

- [ ] **`sponsor_sighash_all_explicit_returns_ok`** — `sponsor.rs` unit
  SIGHASH_ALL explícito deve retornar `Ok(SponsorPrepared)` com PSET enriquecido (não só
  "ausência de SPONSOR_SIGHASH_NOT_ALL").
  _Reforça o assert positivo que o teste existente `sponsor_sighash_not_all` não faz._

### Baixo

- [x] **`esplora_max_response_size_default_64mib`** — `esplora.rs` unit
  `EsploraClientBuilder::new(url, network).build()` sem chamar `.max_response_size()` →
  verificar que `client.max_response_size == 64 * 1024 * 1024`.
  _Verifica que o campo padrão está correto sem chamada explícita ao builder._

---

## Response size cap — candidatos a testes

### Critico

- [x] **`read_body_response_too_large`** — `esplora.rs` unit (tokio)
  Mock server (via helper `serve()` já existente) enviando body maior que o limite configurado
  → `Error::ResponseTooLarge { limit }` com o valor correto em `limit`.
  _Proteção central contra DoS; falha aqui = proteção inexistente._

- [x] **`read_body_within_limit_ok`** — `esplora.rs` unit (tokio)
  Mock server enviando body menor que o limite → `Ok(bytes)` com conteúdo íntegro.
  _Garante que a proteção não quebra o caso normal._

- [x] **`read_body_ignores_content_length`** — `esplora.rs` unit (tokio)
  Mock server enviando `Content-Length: 10` no header mas body real de 1 MiB; limite configurado
  em 100 bytes → `Error::ResponseTooLarge`.
  _Sem esse teste, um server malicioso poderia bypassar o limite com Content-Length mentiroso._

### Alto

- [ ] **`read_error_body_truncated_to_2000_bytes`** — `esplora.rs` unit (tokio)
  Mock server respondendo com 404 e body de 10.000 bytes → verificar que o erro retornado
  contém body com no máximo 2000 bytes lidos (não que o body resultante tem 2000 chars — o
  límite de leitura é 2000 bytes, o snippet final é truncado a 500 chars).
  _Server não pode forçar download de body de erro gigante._

- [ ] **`error_for_status_snippet_truncated_to_500_chars`** — `esplora.rs` unit (tokio)
  Mock server respondendo com 500 e body de 10.000 chars ASCII → campo `body` em
  `EsploraHttpError` tem no máximo 500 chars.
  _Verifica o truncamento final do snippet._

- [ ] **`error_for_status_empty_body_is_none`** — `esplora.rs` unit (tokio)
  Mock server respondendo com 500 e body vazio → campo `body` em `EsploraHttpError` é `None`.
  _Comportamento documentado: `(!snippet.is_empty()).then_some(snippet)`._

### Medio

- [ ] **`esplora_builder_max_response_size_propagated`** — `esplora.rs` unit
  `EsploraClientBuilder::new(url, network).max_response_size(100).build()` →
  `client.max_response_size == 100`.
  _Verifica que o valor configurado pelo caller é de fato propagado para o client._

### Baixo

- [x] **`read_text_invalid_utf8_returns_generic_error`** — `esplora.rs` unit (tokio)
  Mock server enviando bytes não-UTF-8 válidos dentro do limite → erro do tipo
  `Error::Generic` com mensagem contendo "invalid response UTF-8".

---

## Como rodar os testes

```bash
# Unit tests (rapidos, sem Docker)
docker run --rm -v "$(pwd)":/workspace -w /workspace \
  lwk-server:dev-builder /root/.cargo/bin/cargo test -p lwk_wollet --lib

# Integration tests e2e (requerem containers)
docker run --rm -v "$(pwd)":/workspace -w /workspace \
  lwk-server:dev-builder /root/.cargo/bin/cargo test -p lwk_wollet
```

Os testes de `esplora.rs` marcados como `(tokio)` usam `#[tokio::test]` e rodam dentro de `--lib` (são unit tests no mesmo arquivo).
