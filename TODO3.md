# FeeSponsor v1: Dominio LWK (fork + infra + operacao)

> Documento autocontido para implementacao no repositorio do fork `fapinheiro/lwk` e nos scripts de infra do LWK.
> Os dominios Backend e App sao implementados em paralelo e dependem dos **contratos congelados** da secao 3. Nao altere nomes, campos ou codigos de erro sem alinhar com os outros dois dominios.

## 1. Contexto da feature (resumo)
Na Liquid toda transacao paga taxa em L-BTC. O usuario do app tem DEPIX, mas nao tem L-BTC, e nao consegue sacar.

Solucao: o backend patrocina a taxa de ordens patrocinaveis (v1: **withdraw**; futuro: swap) com um UTXO de L-BTC de uma **wallet de sponsor dedicada**. Este dominio nao conhece tipos de ordem: os metodos do fork sao genericos (recebem um outpoint e um endereco de destino).
1. Backend reserva um UTXO de L-BTC e entrega ao app os dados para usa-lo como input externo.
2. O **app** monta a transacao inteira (`lwk_bindings`, `TxBuilder.add_external_utxos`): DEPIX do usuario para o `depositAddress` do parceiro, fee paga pelo UTXO do backend, troco de L-BTC de volta para o backend.
3. Backend valida o PSET, co-assina **somente** o proprio input e devolve.
4. App confere, assina seus inputs e faz o broadcast.

O backend nunca tem as 12 palavras do usuario nem do parceiro.

## 2. Papel deste dominio
- Expor no JSON-RPC do LWK dois metodos novos que hoje nao existem: `wallet_utxo_secrets` e `wallet_sponsor_prepare`.
- Executar as validacoes de seguranca que precisam do PSET parseado em Rust (sighash, `witness_utxo`, issuance, input reservado).
- Subir e operar a instancia do LWK com a wallet de sponsor.
- Manter os procedimentos manuais de UTXO da wallet de sponsor.

Por que o fork e necessario (verificado no codigo do fork, base LWK 0.18):
- `signer_sign` so assina inputs com `bip32_derivation` contendo o fingerprint do signer (`lwk_signer/src/software.rs`). Um UTXO adicionado pelo app como externo nao tem isso. Quem adiciona e `Wollet::add_details()`, e nenhum metodo RPC atual o chama.
- `wallet_utxos` nao retorna blinding factors nem a tx de origem, que o `ExternalUtxo` exige.
- O signer usa o `sighash_type` que vem dentro do PSET (`software.rs` ~ linha 386). Sem validacao, um PSET com `SIGHASH_NONE` permitiria redirecionar o L-BTC do backend.

## 3. Contratos congelados

### 3.1 Convencoes
- JSON-RPC 2.0 sobre HTTP, como os metodos existentes: `{"jsonrpc":"2.0","id":N,"method":"...","params":{...}}`.
- Erros de dominio: objeto `error` padrao do `lwk_tiny_jrpc`, com `message` **iniciando** por um dos codigos abaixo seguido de `: ` e detalhe livre. O backend faz o mapeamento pelo prefixo.
- Blinding factors e asset ids serializados com `to_string()` dos tipos do `elements` (hex no formato que `AssetBlindingFactor::from_str` / `ValueBlindingFactor::from_str` e os construtores `from_string` do `lwk_bindings` aceitam).

### 3.2 `wallet_utxo_secrets`
Request:
```json
{ "name": "sponsor", "txid": "hex", "vout": 1 }
```
Response:
```json
{
  "txid": "hex",
  "vout": 1,
  "tx_hex": "0200000001...",
  "script_pubkey": "hex",
  "asset": "hex asset id",
  "asset_bf": "hex",
  "value": 5000,
  "value_bf": "hex",
  "blinding_key": "hex (32 bytes)",
  "height": 3456789,
  "max_weight_to_satisfy": 271,
  "is_segwit": true
}
```
Regras:
- `blinding_key` = chave privada de blinding do **script** do UTXO, derivada do descriptor CT da wallet (`lwk_common::derive_blinding_key(descriptor, script_pubkey)`, slip77), em hex de 32 bytes. E o campo que o app usa: o `lwk_bindings` publicado (sem a feature `simplicity`) nao tem construtor de `TxOutSecrets` a partir de blinding factors, so `TxOut.unblind(SecretKey)`.
  - Nao permite gastar o UTXO e nao expoe a chave mestra de blinding (derivacao slip77 por script, irreversivel). Revela valor/ativo de qualquer output enviado para aquele mesmo endereco (aceito: wallet de sponsor so tem pequenos valores de L-BTC).
- `asset_bf`/`value_bf` continuam na resposta para diagnostico; o backend nao os repassa ao app.
- O outpoint deve ser um UTXO **nao gasto** da wallet (`wollet.utxos()`), senao `SPONSOR_UTXO_NOT_FOUND`.
- `tx_hex` = transacao de origem serializada (`wollet.transaction(&txid)?.tx`, `elements::encode::serialize_hex`).
- `height` = `null` se nao confirmado.

### 3.3 `wallet_sponsor_prepare`
Request:
```json
{
  "name": "sponsor",
  "pset": "base64",
  "txid": "hex",
  "vout": 1,
  "deposit_address": "lq1..."
}
```
Response:
```json
{
  "pset": "base64 (PSET com add_details aplicado)",
  "txid": "hex (txid da tx nao assinada)",
  "fee": 45,
  "outputs": [
    { "index": 0, "script_pubkey": "hex", "kind": "deposit", "blinding_key_matches": true },
    { "index": 1, "script_pubkey": "hex", "kind": "other",   "blinding_key_matches": null },
    { "index": 2, "script_pubkey": "hex", "kind": "mine",    "blinding_key_matches": null },
    { "index": 3, "script_pubkey": "",    "kind": "fee",     "blinding_key_matches": null }
  ]
}
```
- `kind`: `fee` (script vazio) | `deposit` (script == script do `deposit_address`) | `mine` (script pertence a wallet `name`) | `other`.
- `blinding_key_matches` (so para `kind = deposit`): `true`/`false` se o campo `blinding_key` do output no PSET existe e e/nao e igual a blinding pubkey do `deposit_address`; `null` se o campo nao existe no PSET.
- `fee`: soma dos outputs de fee em L-BTC.

Regras (nesta ordem, primeira falha retorna erro):
1. `deposit_address` parseia e pertence a rede do servidor, senao `SPONSOR_DEPOSIT_ADDRESS_INVALID`.
2. **Identificacao dos inputs da wallet.** Um input e "da wallet" se atender a **qualquer** um dos criterios:
   - **por outpoint (criterio principal):** o `previous_output` (`txid:vout`) e um output conhecido da wallet no cache, **gasto ou nao** (qualquer output de transacao da wallet cujo script pertence a wallet);
   - **por script (verificacao adicional):** o `witness_utxo.script_pubkey`, quando presente, pertence a wallet.
   O conjunto de inputs da wallet deve ser **exatamente** `{txid:vout}` (o reservado presente e nenhum outro), senao `SPONSOR_INPUT_MISMATCH`.
   Motivo: o `witness_utxo` e um dado declarado pelo app. Identificar so pelo script deixaria passar um segundo UTXO do sponsor com `witness_utxo` ausente ou falso.
3. `sighash_type` do input reservado ausente ou `SIGHASH_ALL`, senao `SPONSOR_SIGHASH_NOT_ALL`.
4. `witness_utxo` do input reservado **presente** e identico ao `TxOut` real do UTXO na wallet, senao `SPONSOR_WITNESS_UTXO_MISMATCH` (vale para ausente e para divergente).
5. Input reservado sem issuance/reissuance (`has_issuance()` falso e campos `issuance_*` vazios), senao `SPONSOR_ISSUANCE_NOT_ALLOWED`.
6. Aplica `wollet.add_details(&mut pset)`.
7. **Reforco pos-`add_details`:** nenhum input alem do reservado pode ter `bip32_derivation` com o fingerprint do signer/descriptor da wallet, senao `SPONSOR_INPUT_MISMATCH`. Fecha a copia de dados de derivacao obtidos de um PSET assinado anteriormente (o fingerprint nao e segredo: volta em todo PSET devolvido pelo backend).
8. Classifica os outputs e devolve.

O metodo **nao** assina, **nao** aplica regra de negocio (whitelist, fee maxima) e **nao** persiste nada.

### 3.4 Codigos de erro (prefixo de `message`)
| Codigo | Metodo | Quando |
|---|---|---|
| `SPONSOR_UTXO_NOT_FOUND` | ambos | outpoint nao e UTXO nao gasto da wallet |
| `SPONSOR_DEPOSIT_ADDRESS_INVALID` | prepare | endereco invalido ou de outra rede |
| `SPONSOR_INPUT_MISMATCH` | prepare | inputs da wallet (por outpoint ou script) != exatamente o outpoint; ou outro input com `bip32_derivation` do fingerprint da wallet |
| `SPONSOR_SIGHASH_NOT_ALL` | prepare | sighash do input da wallet != ALL |
| `SPONSOR_WITNESS_UTXO_MISMATCH` | prepare | `witness_utxo` do input reservado ausente ou divergente |
| `SPONSOR_ISSUANCE_NOT_ALLOWED` | prepare | issuance no input da wallet |

PSET que nao parseia ou wallet inexistente: erros genericos existentes do LWK (o backend trata como `PSET_MALFORMED` / falha de LWK).

### 3.5 Metodos existentes que o backend tambem usa (nao alterar)
- `wallet_utxos {name}` -> `{utxos:[{txid,vout,height,script_pubkey,address,asset,value}]}`
- `wallet_address {name, index:null, with_text_qr:false}` -> `{address, index, memo}`
- `wallet_pset_details {name, pset, with_tickers:false}` -> `{txid, has_signatures_from, missing_signatures_from, balance:{asset:i64}, fees:{asset:u64}, issuances, reissuances, warnings}`
- `signer_sign {name, pset}` -> `{pset}`

## 4. Tarefas: fork

### 4.1 Estrutura sugerida
- **Logica em `lwk_wollet`** (unit-testavel, acessa `pub(crate)` como `index()`/`definite_descriptor()`):
  - novo modulo `lwk_wollet/src/sponsor.rs`
  - `impl Wollet { pub fn utxo_secrets(&self, outpoint) -> Result<UtxoSecrets, Error>; pub fn sponsor_prepare(&self, pset, outpoint, deposit: &Address) -> Result<SponsorPrepared, Error> }`
  - variantes novas em `lwk_wollet::Error`, cujo `Display` comeca com o codigo da secao 3.4
- **RPC em `lwk_app`**: handlers finos que chamam os metodos acima.

### 4.2 Checklist
- [ ] Ler `AGENTS.md`/`CLAUDE.md` do repo e seguir as convencoes (formatacao, testes, commits)
- [ ] `lwk_wollet/src/sponsor.rs`: `utxo_secrets` e `sponsor_prepare` com as regras 3.2/3.3
- [ ] `lwk_wollet::Error`: variantes com mensagens prefixadas pelos codigos de 3.4
- [ ] `lwk_rpc_model/src/request.rs`: `WalletUtxoSecrets`, `WalletSponsorPrepare`
- [ ] `lwk_rpc_model/src/response.rs`: `WalletUtxoSecrets`, `WalletSponsorPrepare`, `SponsorOutput`
- [ ] `lwk_app/src/method.rs`: registrar `WalletUtxoSecrets` / `WalletSponsorPrepare` em todos os pontos (enum, schema request, schema response, `FromStr` `"wallet_utxo_secrets"`/`"wallet_sponsor_prepare"`, `Display`)
- [ ] Regenerar `lwk_app/schema.json` conforme a convencao do repo
- [ ] `lwk_app/src/lib.rs`: handlers (lock de estado igual aos handlers existentes, `s.wollets.get_mut(&r.name)?`)
- [ ] `lwk_app/src/client.rs`: `wallet_utxo_secrets(...)`, `wallet_sponsor_prepare(...)`
- [ ] (Opcional) `lwk_cli`: subcomandos `wallet utxo-secrets` e `wallet sponsor-prepare` + `schema.rs`
- [ ] Testes unitarios em `lwk_wollet` (um por regra):
  - [ ] outpoint inexistente / gasto -> `SPONSOR_UTXO_NOT_FOUND`
  - [ ] endereco de outra rede -> `SPONSOR_DEPOSIT_ADDRESS_INVALID`
  - [ ] PSET sem o input reservado / com dois inputs da wallet -> `SPONSOR_INPUT_MISMATCH`
  - [ ] input extra de UTXO do sponsor **sem** `witness_utxo` (identificado por outpoint) -> `SPONSOR_INPUT_MISMATCH`
  - [ ] input extra de UTXO do sponsor com `witness_utxo` de **script falso** -> `SPONSOR_INPUT_MISMATCH`
  - [ ] input extra de UTXO **ja gasto** do sponsor -> `SPONSOR_INPUT_MISMATCH`
  - [ ] input nao relacionado com `bip32_derivation` copiado (fingerprint da wallet) -> `SPONSOR_INPUT_MISMATCH`
  - [ ] sighash `NONE`, `SINGLE`, `ALL|ANYONECANPAY` -> `SPONSOR_SIGHASH_NOT_ALL`; ausente e `ALL` -> ok
  - [ ] `witness_utxo` alterado -> `SPONSOR_WITNESS_UTXO_MISMATCH`
  - [ ] `witness_utxo` **ausente** no input reservado -> `SPONSOR_WITNESS_UTXO_MISMATCH`
  - [ ] issuance no input -> `SPONSOR_ISSUANCE_NOT_ALLOWED`
  - [ ] classificacao correta de `fee`/`deposit`/`mine`/`other` e `blinding_key_matches`
  - [ ] `wallet_utxo_secrets`: `TxOut.unblind(blinding_key)` do output reproduz exatamente `asset`, `value`, `asset_bf`, `value_bf` devolvidos
- [ ] Teste de integracao (padrao dos testes existentes com regtest/containers):
  1. wallet A (sponsor) com L-BTC; wallet B (usuario) com um asset emitido (simula DEPIX) e sem L-BTC
  2. B monta PSET: `set_wallet_utxos(utxos do asset)`, `add_external_utxos([UTXO de A via utxo_secrets])`, recipient do asset para endereco C, `drain_lbtc_to(endereco de A)`, `finish`
  3. RPC `wallet_sponsor_prepare` -> `signer_sign` de A -> `wallet_pset_details` de A mostra assinatura de A e `balance[L-BTC] == -fee`
  4. B assina, finaliza e faz broadcast -> confirma
- [ ] Teste de integracao negativo: mesmo fluxo com sighash `NONE` no input de A -> erro
- [ ] Confirmar e documentar se o TxBuilder preenche `blinding_key` nos outputs do PSET (define se `blinding_key_matches` vem `null` ou booleano)
- [ ] Confirmar que o `txid` (tx nao assinada) nao muda apos assinaturas (o app usa isso)
- [ ] Teste de compatibilidade: PSET montado com `lwk_bindings` **0.19.0** (versao dos apps), incluindo `feeRate` customizado, aceito por `wallet_sponsor_prepare` e assinado por `signer_sign` do fork (0.18). Obrigatorio antes de liberar
- [ ] Atualizar `LWK_REF` do Dockerfile e registrar no changelog interno (`Wallet.md`, secao 13)

## 5. Tarefas: infra

### 5.1 Uma instancia ou duas?
Um unico processo LWK carrega varias wallets e signers, cada um com seu nome (`wallet load --wallet sponsor`, `signer load-software --signer sponsor`). Tecnicamente uma instancia funciona.

**Recomendacao: duas instancias separadas** (`lwk-server` para recebimento, `lwk-sponsor` para sponsor), mesma imagem:
- **Raio de dano isolado:** seeds em `state.json` separados, volumes e `.env` separados.
- **Runbook de incidente independente:** `signer unload` / parar o sponsor sem afetar o recebimento, e vice-versa.
- **Restart independente:** o watchdog reinicia uma sem derrubar a outra; o estado do LWK fica num unico mutex por processo, e o scan de uma wallet nao segura requisicoes da outra.
- **k3s:** vira 1:1 dois StatefulSets.

Custo: segunda porta, volume, `.env`, unit systemd e watchdog. O backend ja tera uma URL propria para o sponsor (`lwk.sponsor.url`), entao funciona com qualquer das duas opcoes (com instancia unica, basta apontar as duas URLs para o mesmo servidor).

### 5.2 Ponto de atencao encontrado no deploy atual
O `.env` define `LWK_MNEMONICS`, mas o `lwk_cli` **nao le essa variavel**. O `--mnemonic` do `signer load-software` nao tem `env`, e as variaveis que o clap le teriam outros nomes. O signer provavelmente foi carregado uma vez via RPC com `--persist true`. Nesse caso a seed esta em texto puro em `HOST_VOLUME/state.json`.
- [ ] Confirmar onde a seed de recebimento realmente vem (`state.json` no volume?)
- [ ] Se `LWK_MNEMONICS` nao e usada, remove-la do `.env` (e o unico outro lugar com a seed em texto puro)
- [ ] Tratar `HOST_VOLUME` como secret: 750 `tappix`, fora de backups nao criptografados

### 5.3 Checklist (duas instancias)
- [ ] Parametrizar `deploy.sh` por instancia (ex.: `./deploy.sh <version> <instance> [flags]` ou arquivo `instances/<nome>.conf`):

  | Variavel | recebimento | sponsor |
  |---|---|---|
  | `SERVICE_NAME` | `lwk-server.service` | `lwk-sponsor.service` |
  | `CONTAINER_NAME` | `lwk-server` | `lwk-sponsor` |
  | `DEPLOY_DIR` / `.env` | `/opt/tappix/infra/lwk` | `/opt/tappix/infra/lwk/sponsor` |
  | `HOST_VOLUME` | `/var/lib/tappix/lwk/dataprd` (atual, nao mover) | `/var/lib/tappix/lwk/sponsor/dataprd` |
  | `HOST_PORT` | `127.0.0.1:32112` | `127.0.0.1:32113` |
  | `APP_LOG_DIR` | `/var/log/tappix/infra/lwk` | `/var/log/tappix/infra/lwk/sponsor` |
  | wallet do healthcheck | `w1` | `sponsor` |

  Atencao aos diretorios aninhados:
  - `/opt/tappix/infra/lwk/sponsor` fica dentro do `DEPLOY_DIR` do recebimento. O deploy do recebimento nao pode apagar nem sobrescrever subpastas (hoje so faz `chown`/`chmod` no diretorio, ok).
  - `/var/lib/tappix/lwk/sponsor/dataprd` fica ao lado de `/var/lib/tappix/lwk/dataprd` (nao dentro dele). Os dois `state.json` ficam separados.
  - O glob atual do logrotate (`/var/log/tappix/infra/lwk/*.log`) **nao** inclui a subpasta `sponsor/`.

- [ ] Watchdog parametrizado: `CONTAINER_NAME`, `SERVICE_NAME`, `SECRETS_FILE`, `LOG_FILE` por instancia (unit/timer `lwk-sponsor-watchdog.*`)
- [ ] Sudoers: incluir `systemctl restart lwk-sponsor.service` para `tappix`
- [ ] logrotate: adicionar `/var/log/tappix/infra/lwk/sponsor/*.log` (no mesmo arquivo `/etc/logrotate.d/tappix-lwk` ou num arquivo proprio)
- [ ] A imagem e a mesma (`lwk-server:<version>-prd`): build uma vez, deploy nas duas instancias
- [ ] Ambas as portas apenas em `127.0.0.1` (ja e o caso). Validar com nmap a partir de fora do host: 32112 e 32113 fechadas
- [ ] Gerar a seed da wallet de sponsor **nova** (nao derivar da de recebimento); backup offline
- [ ] Carregar signer e wallet de sponsor (uma vez; persistido em `state.json`). Use espaco inicial no comando ou `read -s`, para a seed nao ficar no historico do shell:
  ```bash
  docker exec -it lwk-sponsor lwk_cli --network mainnet --addr 127.0.0.1:32112 \
    signer load-software --signer sponsor --persist true --mnemonic "$SPONSOR_MNEMONIC"
  DESC=$(docker exec lwk-sponsor lwk_cli --network mainnet --addr 127.0.0.1:32112 \
    signer singlesig-desc --signer sponsor --descriptor-blinding-key slip77 --kind wpkh | jq -r .descriptor)
  docker exec lwk-sponsor lwk_cli --network mainnet --addr 127.0.0.1:32112 \
    wallet load --wallet sponsor -d "$DESC"
  ```
  (dentro do container a porta e 32112; do host, 32113)
- [ ] Conferir que o descriptor carregado e multipath completo (`.../<0;1>/*`): `wallet details --wallet sponsor`. Sem a cadeia interna, o troco dos envios patrocinados fica invisivel para a wallet
- [ ] Registrar no `system_config` do backend: `sponsor_wallet_name=sponsor`, `sponsor_signer_name=sponsor`; URL do backend `lwk.sponsor.url=http://127.0.0.1:32113`
- [ ] Garantir que o job de consolidacao horaria **nunca** aponte para a instancia/wallet de sponsor
- [ ] Fan-out inicial (`Wallet.md`, secao 6)

### 5.4 k3s (futuro)
- Um `StatefulSet` por instancia, `replicas: 1` (o LWK guarda estado em disco e nao escala horizontalmente), cada um com PVC proprio.
- Seed via `Secret` montado so no bootstrap, ou carregada uma vez via RPC; o PVC continua sendo secret (ver 5.2).
- `Service` `ClusterIP` (sem NodePort/Ingress) + `NetworkPolicy` permitindo entrada apenas dos pods do backend.
- Liveness/readiness com o mesmo `wallet_balance` do healthcheck atual; o watchdog de sync vira um sidecar ou `CronJob`.

## 6. Criterios de aceite do dominio
- [ ] `wallet_utxo_secrets` e `wallet_sponsor_prepare` respondem conforme a secao 3, com os codigos de erro da 3.4
- [ ] Todos os testes unitarios e de integracao da 4.2 passam
- [ ] O PSET retornado por `wallet_sponsor_prepare` e assinado por `signer_sign` (assinatura do signer de sponsor presente em `wallet_pset_details`)
- [ ] Instancia de sponsor rodando, com healthcheck e watchdog proprios, porta so em loopback (nmap)
- [ ] Wallet de sponsor carregada, com UTXOs do fan-out inicial visiveis em `wallet_utxos`

## 7. Operacao
Os procedimentos manuais da wallet de sponsor (dimensionamento, fan-out, consolidacao, rotinas diaria/semanal/mensal/trimestral, reposicao de sponsor, supply chain do fork, runbook de incidente e registro de operacoes) estao no handbook **`Wallet.md`**.

Itens deste dominio que dependem do handbook:
- Fan-out inicial apos a instalacao: `Wallet.md`, secao 6.
- Atualizacao do fork / `LWK_REF`: `Wallet.md`, secao 13.
- Script de verificacao diaria (`sponsor-check.sh`): `Wallet.md`, secao 8.1 (instalar em `/opt/tappix/infra/lwk/sponsor/`).

## 8. Instrucoes para o agente
- Escopo: somente o repositorio do fork e os scripts de infra do LWK. Nao implementar nada do backend Java nem do app.
- Os contratos da secao 3 sao fixos. Se encontrar um motivo tecnico para muda-los, pare e reporte antes de alterar.
- Nao enfraquecer nenhuma regra da 3.3 por conveniencia de teste.
- Seeds reais nunca vao para o repositorio, testes ou logs.
