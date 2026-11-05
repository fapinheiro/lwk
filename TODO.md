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
