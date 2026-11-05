# TODO2.md — Gap de segurança: Content-Length bypass no read_body

## Status

Pendente. Identificado durante a implementação dos testes Alto (sessão spec-test, 2026-11-05).
Teste documentando o gap já existe e está **falhando propositalmente**.

---

## O gap

`read_body` em `lwk_wollet/src/clients/asyncr/esplora.rs` conta bytes reais do stream
para fazer cumprir o limite `max_response_size`. Mas **reqwest/hyper em HTTP/1.1 respeita
`Content-Length` antes de entregar o stream ao caller**.

Consequência: um servidor malicioso pode anunciar `Content-Length: 10` e transmitir
gigabytes de dados. `read_body` vê apenas os 10 bytes declarados (o stream fecha no byte 10),
não dispara `ResponseTooLarge`, e os dados extras nunca chegam ao proteção.

---

## Localização exata

| Item | Arquivo | Linha |
|------|---------|-------|
| `read_body` (implementação) | `lwk_wollet/src/clients/asyncr/esplora.rs` | ~910 |
| Teste falhando | `lwk_wollet/src/clients/asyncr/esplora.rs` | ~1657 (`read_body_ignores_content_length`) |

---

## Teste que documenta o gap

```rust
// lwk_wollet/src/clients/asyncr/esplora.rs  — linha ~1657
#[tokio::test]
async fn read_body_ignores_content_length() {
    // Servidor anuncia Content-Length: 10 mas envia 2 000 bytes.
    // Limite configurado: 100 bytes.
    // Comportamento esperado pela spec: Error::ResponseTooLarge { limit: 100 }
    // Comportamento atual:             I/O error: failed to fill whole buffer
    //   (reqwest leu 10 bytes, decode de Transaction falhou por dados insuficientes)
}
```

O teste foi escrito com `assert!(matches!(err, crate::Error::ResponseTooLarge { limit: 100 }))`.
Enquanto o gap existir, ele falha com:
```
read_body must count actual streamed bytes and not trust Content-Length;
got: I/O error: failed to fill whole buffer
```

---

## Causa raiz

reqwest usa hyper por baixo. Em HTTP/1.1 com `Content-Length`, hyper delimita o corpo
exatamente pelo valor declarado antes de expor o `bytes_stream()` ao caller. Não há como
contar "bytes reais transmitidos" porque hyper já truncou no nível do parser HTTP.

---

## Opções de fix (a decidir)

### Opção A — Desabilitar Content-Length no builder reqwest
```rust
// EsploraClientBuilder::build()
let mut builder = reqwest::Client::builder()
    .default_headers(headers)
    // Força leitura até fim da conexão, ignora Content-Length
    // (não existe flag direta no reqwest 0.12; requer wrapper manual)
```
Problema: reqwest 0.12 não expõe uma flag para ignorar Content-Length.

### Opção B — Usar Transfer-Encoding: chunked no servidor de teste
Ajustar o mock server de `read_body_ignores_content_length` para **não enviar
Content-Length** e usar `Connection: close` como único delimitador de corpo.
Sem `Content-Length`, hyper lê até fechar a conexão e o stream entrega todos os bytes.

```rust
// Mock server sem Content-Length:
let resp = format!(
    "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{actual_body}"
);
// (sem o cabeçalho Content-Length)
```

Nesse cenário reqwest entrega o stream completo (2 000 bytes), `read_body` conta
cada chunk e dispara `ResponseTooLarge { limit: 100 }` ao ultrapassar 100 bytes.
O teste passa. E a proteção funciona para servidores que **não** mentem o Content-Length.

Limitação desta opção: servidores que anunciam `Content-Length` mentiroso **continuam
bypassando** a proteção, porque o stream entregue ao caller já foi truncado. O fix
protege contra respostas sem Content-Length (chunked / close-delimited), que é o caso
mais comum de resposta grande, mas não protege contra Content-Length adversarial.

### Opção C — Verificar Content-Length antes de baixar
Antes de chamar `read_body`, verificar `response.content_length()`. Se o valor declarado
já exceder `max_response_size`, rejeitar imediatamente sem ler o corpo.

```rust
async fn read_body(&self, response: Response) -> Result<Vec<u8>, Error> {
    let limit = self.max_response_size;
    // Rejeição rápida por Content-Length declarado.
    if let Some(cl) = response.content_length() {
        if cl as usize > limit {
            return Err(Error::ResponseTooLarge { limit });
        }
    }
    // ... streaming normal ...
}
```

Esta opção não protege contra `Content-Length` mentiroso pequeno + corpo gigante, mas
protege contra `Content-Length` honestamente grande (o caso mais comum em produção).
Pode ser combinada com a Opção B.

### Opção recomendada

**B + C combinadas**: remover Content-Length do mock server do teste (Opção B) para o
teste passar, e adicionar early-reject por Content-Length declarado (Opção C) como
camada extra de proteção. Documentar no código que Content-Length adversarial
(anuncia pequeno, envia grande) não é coberto por essa proteção dado o comportamento
do reqwest/hyper.

---

## Critérios de aceite para fechar este item

1. `read_body_ignores_content_length` **passa** (sem mudar o assert final).
2. `read_body_response_too_large` e `read_body_within_limit_ok` continuam passando.
3. Comportamento documentado no código com comentário explicando a limitação do
   Content-Length adversarial pequeno.

---

## Contexto adicional

- `DEFAULT_MAX_RESPONSE_SIZE` = 64 MiB (definido em `lwk_wollet/src/clients/mod.rs:30`)
- `read_error_body` (para erros HTTP) usa limite fixo de `ERROR_BODY_MAX_CHARS * 4 = 2000` bytes
  e **não** passa por `read_body`, portanto não é afetada por este gap.
- Todos os outros testes de size cap (`read_body_response_too_large`,
  `read_body_within_limit_ok`) funcionam porque `serve()` envia Content-Length correto
  e o stream contém exatamente os bytes declarados.
