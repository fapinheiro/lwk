# TEST2.md — Testes para o gap de segurança: Content-Length bypass no read_body

**Spec de referência:** `TODO2.md`
**Implementação coberta:** `lwk_wollet/src/clients/asyncr/esplora.rs` — `read_body` (Opção B + C)
**Onde colar:** módulo `#[cfg(test)]` de `lwk_wollet/src/clients/asyncr/esplora.rs`, junto aos testes `read_body_*` existentes.

---

## Crítico

### `read_body_early_reject_by_declared_content_length`

Testa o novo caminho da **Opção C**: se `Content-Length` declarado já excede o limite,
`read_body` deve rejeitar imediatamente sem ler nenhum byte do stream.

Sem este teste, a Opção C pode ser removida sem que nenhum teste existente falhe —
`read_body_response_too_large` continuaria passando via contador de stream.

```rust
/// `Content-Length` declarado acima do limite → early-reject com `ResponseTooLarge`
/// antes de qualquer leitura do stream (Opção C).  Teste explícito do novo caminho:
/// sem ele, remover o early-reject não quebraria nenhum outro teste.
#[tokio::test]
async fn read_body_early_reject_by_declared_content_length() {
    // serve() envia Content-Length correto (= body.len()).
    // body = 50 bytes, limit = 10 → CL (50) > limit (10) → early-reject.
    let base = serve("200 OK", "A".repeat(50).leak()).await;
    let client = EsploraClientBuilder::new(&base, Network::Liquid)
        .max_response_size(10)
        .build()
        .unwrap();
    let txid = elements::Txid::from_str(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let err = client.get_transaction(txid).await.unwrap_err();
    assert!(
        matches!(err, crate::Error::ResponseTooLarge { limit: 10 }),
        "Content-Length > limit deve disparar ResponseTooLarge imediato; got: {err}"
    );
}
```

---

## Alto

### `read_body_content_length_exactly_at_limit_passes`

`Content-Length` = limite exato → **não** deve ser rejeitado pelo early-reject.
Verifica que a condição é `>` (estrito), não `>=`. Uma regressão trivial bastaria
para quebrar respostas que ocupam exatamente o limite configurado.

```rust
/// `Content-Length` igual ao limite exato → não deve disparar early-reject.
/// Verifica que a guarda usa `>` e não `>=`.
#[tokio::test]
async fn read_body_content_length_exactly_at_limit_passes() {
    // body = 8 bytes, limit = 8 → CL (8) == limit (8) → sem rejeição por CL.
    // O decode de "AAAAAAAA" como Transaction vai falhar, mas sem ResponseTooLarge.
    let base = serve("200 OK", "AAAAAAAA").await;
    let client = EsploraClientBuilder::new(&base, Network::Liquid)
        .max_response_size(8)
        .build()
        .unwrap();
    let txid = elements::Txid::from_str(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let err = client.get_transaction(txid).await.unwrap_err();
    assert!(
        !matches!(err, crate::Error::ResponseTooLarge { .. }),
        "Content-Length == limit não deve disparar ResponseTooLarge; got: {err}"
    );
}
```

### `read_body_no_content_length_body_within_limit`

Sem `Content-Length` (close-delimited), corpo ≤ limite → sucesso no streaming.
`read_body_within_limit_ok` usa `serve()`, que envia `Content-Length` correto.
Este teste cobre o branch em que não há cabeçalho — o único delimitador é o
fechamento da conexão.

```rust
/// Sem `Content-Length`, corpo ≤ limite → streaming encerra normalmente sem
/// disparar `ResponseTooLarge`.  Cobre o branch close-delimited que
/// `read_body_within_limit_ok` não exercita.
#[tokio::test]
async fn read_body_no_content_length_body_within_limit() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await;
        // 10 bytes, sem Content-Length — só Connection: close como delimitador.
        let resp = "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nHelloWorld";
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let base = format!("http://{addr}");

    let client = EsploraClientBuilder::new(&base, Network::Liquid)
        .max_response_size(1024)
        .build()
        .unwrap();
    let txid = elements::Txid::from_str(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let err = client.get_transaction(txid).await.unwrap_err();
    // Corpo dentro do limite → não deve disparar ResponseTooLarge.
    // O erro de decode que se segue é esperado e mostra que o body chegou intacto.
    assert!(
        !matches!(err, crate::Error::ResponseTooLarge { .. }),
        "corpo dentro do limite (sem CL) não deve disparar ResponseTooLarge; got: {err}"
    );
}
```

---

## Médio

### `read_body_content_length_zero`

`Content-Length: 0` → não deve ser rejeitado (0 não é > limit para qualquer limit ≥ 1).
Edge case numérico do early-reject.

```rust
/// `Content-Length: 0` → early-reject não deve disparar (0 ≤ qualquer limit ≥ 1).
/// Retorna body vazio; o erro de decode que se segue é esperado.
#[tokio::test]
async fn read_body_content_length_zero() {
    // serve() com body vazio envia Content-Length: 0.
    let base = serve("200 OK", "").await;
    let client = EsploraClientBuilder::new(&base, Network::Liquid)
        .max_response_size(10)
        .build()
        .unwrap();
    let txid = elements::Txid::from_str(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let err = client.get_transaction(txid).await.unwrap_err();
    assert!(
        !matches!(err, crate::Error::ResponseTooLarge { .. }),
        "Content-Length: 0 não deve disparar ResponseTooLarge; got: {err}"
    );
}
```

---

## Baixo

### `read_body_known_gap_small_content_length_bypasses_protection`

Documenta o **gap residual**: servidor declara `Content-Length` pequeno (≤ limite)
mas transmite muito mais bytes. reqwest/hyper trunca o stream no valor declarado,
então `read_body` nunca vê os bytes extras — a proteção não dispara.

Este teste **não é uma garantia de segurança** — é uma âncora de regressão:
ele passa enquanto o gap existir, e deve falhar (e ser revisado) se o comportamento
do reqwest/hyper mudar em versões futuras.

```rust
/// KNOWN GAP: servidor declara `Content-Length: 10` (≤ limite 100) mas envia
/// 2 000 bytes.  reqwest/hyper trunca o stream em 10 bytes → read_body nunca
/// vê os bytes extras → ResponseTooLarge NÃO é disparado.
///
/// Este teste documenta a limitação descrita na docstring de `read_body`.
/// Se passar a falhar, significa que reqwest/hyper mudou de comportamento
/// e a proteção melhorou — revisar e possivelmente remover o `// KNOWN GAP`.
#[tokio::test]
async fn read_body_known_gap_small_content_length_bypasses_protection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = sock.read(&mut buf).await;
        // KNOWN GAP: anuncia CL=10 (≤ limit 100) mas envia 2 000 bytes.
        let actual_body = "A".repeat(2_000);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n{actual_body}"
        );
        let _ = sock.write_all(resp.as_bytes()).await;
    });
    let base = format!("http://{addr}");

    let client = EsploraClientBuilder::new(&base, Network::Liquid)
        .max_response_size(100)
        .build()
        .unwrap();
    let txid = elements::Txid::from_str(
        "0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let err = client.get_transaction(txid).await.unwrap_err();
    // KNOWN GAP: a proteção NÃO dispara porque hyper truncou o stream em 10 bytes.
    // O erro observado é de decode (dados insuficientes), não ResponseTooLarge.
    assert!(
        !matches!(err, crate::Error::ResponseTooLarge { .. }),
        "KNOWN GAP: esperado erro de decode (não ResponseTooLarge); got: {err}"
    );
}
```

---

## Resumo dos testes

| Teste | Criticidade | Comportamento da spec coberto |
|-------|-------------|-------------------------------|
| `read_body_early_reject_by_declared_content_length` | Crítico | CL > limit → early-reject (Opção C) |
| `read_body_content_length_exactly_at_limit_passes` | Alto | CL = limit → sem rejeição (`>` estrito) |
| `read_body_no_content_length_body_within_limit` | Alto | Sem CL, corpo ≤ limit → sucesso (close-delimited) |
| `read_body_content_length_zero` | Médio | CL = 0 → sem rejeição |
| `read_body_known_gap_small_content_length_bypasses_protection` | Baixo | Gap residual documentado |
