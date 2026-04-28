# Контракт API: Сервис Репутации Спамеров (v1)

Этот документ описывает интерфейс внешнего сервиса, который агрегирует данные о спамерах для нескольких инстансов бота.

## Общие положения
- **Формат данных:** JSON
- **Кодировка:** UTF-8
- **Базовый путь:** `/v1/reputation`

## 1. Типы данных

### SpamType (String Enum)
Категория нарушения:
- `crypto_scam` — крипто-мошенничество.
- `dating_bot` — боты знакомств.
- `casino_ads` — гемблинг.
- `channel_promotion` — спам ссылками на каналы.
- `adult_content` — 18+ контент.
- `unknown_spam` — прочий спам.

### SpammerRecord (Object)
```json
{
  "user_id": 12345678,
  "phash": "a1b2c3d4e5f6g7h8", 
  "bio_text": "text",
  "spam_type": "crypto_scam",
  "confidence": 95,
  "first_detected_at": "2024-05-20T12:00:00Z",
  "source_bot_id": "bot_1"
}
```

## 2. Эндпоинты

### 2.1 Проверка пользователя
**Метод:** `POST /check`

**Запрос:**
```json
{
  "user_id": 12345678,
  "phash": "optional_hash",
  "bio_text": "optional_bio"
}
```

**Ответ (200 OK):**
```json
{
  "is_spammer": true,
  "record": { ... },
  "action_recommended": "ban",
  "similarity_score": 0.98
}
```

### 2.2 Репорт спамера
**Метод:** `POST /report`

**Запрос:**
```json
{
  "user_id": 12345678,
  "phash": "hash",
  "bio_text": "...",
  "spam_type": "casino_ads",
  "source_bot_id": "bot_1",
  "evidence_message": "..."
}
```

**Ответ (201 Created):**
```json
{ "status": "recorded", "global_id": "uuid" }
```

### 2.3 Горячий список (Cache Warming)
**Метод:** `GET /hotlist?limit=1000`

**Ответ (200 OK):**
```json
{
  "count": 1000,
  "records": [ { ... }, ... ]
}
```
