# LWK fork: cherry-picks de segurança do upstream

Branch: `feat/feesponsor`

## Contexto
O fork foi criado há ~2 meses. O upstream (Blockstream/lwk) acumulou 3 commits
relevantes para produção que não conflitam com o que implementamos.
Fazer antes do próximo deploy.

## Tarefas

- [ ] Cherry-pick `a9e9 0ea42` — `deps: update rustls-webpki and webpki-roots`
  - Corrige RUSTSEC-2026-0382 e RUSTSEC-2026-0381 (TLS certificate validation)
  - Apenas Cargo.lock, sem conflito

- [ ] Cherry-pick `c8abcbbd9` — `wollet: limit response size`
  - Cap de 64 MiB por resposta HTTP; previne DoS por servidor malicioso
  - Toca clients/esplora — sem conflito com sponsor.rs

- [ ] Cherry-pick `5ade691d8` — `wollet: cap error body size`
  - Le no maximo 2000 bytes de error body (clients/oauth.rs)
  - Sem conflito

- [ ] Rodar testes após os cherry-picks:
  ```
  docker run --rm -v <path>/lwk:/workspace -w /workspace \
    lwk-server:dev-builder /root/.cargo/bin/cargo test -p lwk_wollet --lib
  ```

- [ ] Novo commit no branch `feat/sync-upstream-04-11-2026`

- [ ] git push para o fork remoto

- [ ] Atualizar `LWK_REF` no `DockerfilePrd` com o novo hash

---

# FeeSponsor v1: itens pendentes

Implementação base já feita (spec 01-LWK-fork-e-infra, seções 3–4):
`lwk_wollet/src/sponsor.rs`, erros, rpc_model, lwk_app handlers/client/method.
Itens abaixo são os que faltam para completar os critérios de aceite (seção 6).

## Testes de integração (bloqueadores de release)

- [x] **Teste positivo** (`test_sponsor_fee_flow`) em `lwk_wollet/tests/e2e.rs`:
  1. Wallet A (sponsor) com L-BTC; wallet B (usuário) com asset emitido e sem L-BTC
  2. A chama `utxo_secrets` → B constrói PSET com `add_external_utxos` + `drain_lbtc_to(addr_A)`
  3. A chama `sponsor_prepare` → signer A assina → verificar assinatura de A presente
  4. B assina, finaliza, faz broadcast → confirma

- [x] **Teste negativo** (`test_sponsor_sighash_none_rejected`) em `lwk_wollet/tests/e2e.rs`:
  - Mesmo fluxo, mas seta `sighash_type = NONE` no input de A antes de chamar `sponsor_prepare`
  - Deve retornar `SPONSOR_SIGHASH_NOT_ALL`

## Compatibilidade (bloqueador de release)

- [ ] Testar que PSET montado com `lwk_bindings` **0.19.0** (versão dos apps) é aceito por
  `wallet_sponsor_prepare` e assinado por `signer_sign` do fork (0.18). Testar manualmente
  ou com script de compatibilidade antes de liberar.

## Infra (fora do repositório do fork)

- [ ] `DockerfilePrd` (infra): atualizar `LWK_REF` com o hash final após todos os testes passarem
- [ ] `Wallet.md` seção 13: registrar atualização do fork no changelog interno
- [ ] Instância `lwk-sponsor` parametrizada em `deploy.sh` (ver spec seção 5.3)
- [ ] Watchdog `lwk-sponsor-watchdog.*`, sudoers, logrotate para a instância sponsor
- [ ] Gerar seed da wallet sponsor (nova, não derivada da de recebimento) + backup offline
- [ ] Carregar signer + wallet sponsor uma vez (`persist true`; persiste em `state.json`)
- [ ] Confirmar descriptor multipath completo (`/<0;1>/*`) via `wallet details --wallet sponsor`
- [ ] Registrar no `system_config` do backend: `sponsor_wallet_name`, `sponsor_signer_name`, `lwk.sponsor.url`
- [ ] Garantir que job de consolidação horária nunca aponte para wallet/instância sponsor
- [ ] Fan-out inicial (`Wallet.md` seção 6)
- [ ] Validar portas 32112 e 32113 fechadas externamente (nmap a partir de fora do host)
- [ ] Confirmar onde a seed de recebimento vem (`state.json` no volume?) — ver spec seção 5.2
- [ ] Se `LWK_MNEMONICS` não é usada pelo CLI, removê-la do `.env`
