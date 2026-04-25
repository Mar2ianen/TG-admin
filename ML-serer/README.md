# ml-server

ML-сервер с embeddings и LLM провайдерами. 100% Rust (Candle для локальных embeddings).

## Возможности

- **Embeddings**: локально через Candle BERT (без внешних API)
- **LLM**: OpenRouter → Groq → Cloudflare → Google (fallback цепочка)

## API Endpoints

### GET /health
Health check сервера.

```bash
curl http://localhost:11434/health
```
Ответ:
```json
{"status":"ok","provider":"local","model":"sentence-transformers/all-MiniLM-L6-v2"}
```

### POST /v1/embeddings
Получить embeddings текста (локально, Candle).

```bash
curl -X POST http://localhost:11434/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"input": ["Hello world", "Test sentence"]}'
```
Ответ:
```json
{
  "object": "list",
  "data": [
    {"object":"embedding","embedding":[...],"index":0},
    {"object":"embedding","embedding":[...],"index":1}
  ],
  "model": "sentence-transformers/all-MiniLM-L6-v2",
  "usage": {"prompt_tokens":2,"total_tokens":2}
}
```

### POST /v1/chat/completions
LLM чат через внешние провайдеры.

```bash
curl -X POST http://localhost:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "messages": [{"role": "user", "content": "Hi"}],
    "model": "meta-llama/llama-3.1-70b-instruct",
    "max_tokens": 50
  }'
```
Ответ:
```json
{
  "id": "chatcmpl-...",
  "object": "chat.completion",
  "created": 1234567890,
  "model": "meta-llama/llama-3.1-70b-instruct",
  "choices": [
    {
      "index": 0,
      "message": {"role": "assistant", "content": "Hello! How can I help?"},
      "finish_reason": "stop"
    }
  ],
  "usage": {"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}
}
```

### GET /v1/models
Список доступных моделей.

```bash
curl http://localhost:11434/v1/models
```

## Переменные окружения

| Переменная | Описание | По умолчанию |
|------------|-----------|-------------|
| `PORT` | Порт сервера | 11434 |
| `HOST` | Хост | 0.0.0.0 |
| `MODEL` | Модель для embeddings | sentence-transformers/all-MiniLM-L6-v2 |

### API Keys (опционально)

```bash
# OpenRouter (рекомендуется - много бесплатных лимитов)
OPENROUTER_API_KEY=sk-or-v1-...

# Groq (быстрый, бесплатные лимиты)
GROQ_API_KEY=gsk_...

# Cloudflare Workers AI
CLOUDFLARE_API_KEY=cfut_...
CLOUDFLARE_ACCOUNT_ID=...

# Google Gemini
GOOGLE_API_KEY=AIzaSy...
```

## Доступные модели

### Embeddings
- `sentence-transformers/all-MiniLM-L6-v2` (локально, бесплатно)

### LLM

**OpenRouter** (приоритет 1):
- `meta-llama/llama-3.1-70b-instruct`
- `meta-llama/llama-3.1-8b-instruct`
- `qwen/qwen-2.5-72b-instruct`
- И ещё 100+ моделей

**Groq** (приоритет 2):
- `llama-3.3-70b-versatile`
- `llama-3.1-8b-instant`
- `mixtral-8x7b-instruct`

**Cloudflare** (приоритет 3):
- `@cf/meta/llama-3.1-70b-instruct`
- `@cf/meta/llama-3.1-8b-instruct`

## Запуск

```bash
# Локально
MODEL=sentence-transformers/all-MiniLM-L6-v2 ./ml-server

# С API ключами
OPENROUTER_API_KEY=sk-or-v1-... \
GROQ_API_KEY=gsk_... \
CLOUDFLARE_API_KEY=cfut_... \
CLOUDFLARE_ACCOUNT_ID=3d2871c5aacb7cba9ce94bbeda9a19be \
./ml-server
```

## Docker

```bash
podman build -t ml-server .
podman run -d -p 11434:11434 \
  -e OPENROUTER_API_KEY=sk-or-v1-... \
  -e GROQ_API_KEY=gsk_... \
  -e CLOUDFLARE_API_KEY=cfut_... \
  -e CLOUDFLARE_ACCOUNT_ID=3d2871c5aacb7cba9ce94bbeda9a19be \
  ml-server
```

## Fallback логика

1. **Embeddings** - всегда локально (Candle)
2. **LLM** - пробует провайдеров по порядку:
   - OpenRouter → Groq → Cloudflare → Google

## Архитектура

```
/v1/embeddings  ──────► Local (Candle BERT)
/v1/chat/completions ─► OpenRouter ► Groq ► Cloudflare ► Google
```

## Тестирование

```bash
# Health
curl http://localhost:11434/health

# Embeddings
curl -X POST http://localhost:11434/v1/embeddings \
  -H "Content-Type: application/json" \
  -d '{"input": ["test"]}'

# Chat
curl -X POST http://localhost:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hi"}],"model":"meta-llama/llama-3.1-70b-instruct"}'
```