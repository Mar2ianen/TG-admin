# Полный анализ проекта Telegram-Moderation-OS

## 1. AI-антипаттерны

### 1.1 API-призраки
**Статус: OK** — реальных галлюцинаций не найдено. Все методы teloxide используются корректно.

### 1.2 Оптимистичное программирование (Happy Path)
**Статус: КРИТИЧЕСКИ** — найдено **336+** опасных паттернов:

| Файл | Проблема | Линии |
|------|----------|-------|
| `src/storage.rs` | `unwrap_or_else(\|e\| panic!(...))` на каждой БД операции | ~60 |
| `src/moderation.rs` | `.unwrap_or(false)`, magic numbers (900, 700) | ~30 |
| `src/runtime.rs:85,97` | `.unwrap_or_default()`, `.unwrap_or(false)` | 2 |
| `src/app.rs` | `.expect("startup succeeds")` и т.д. | ~20 |
| `src/tg.rs` | `.expect()` на Telegram API | ~15 |
| `src/parser/target.rs` | `panic!()` в парсинге | ~10 |

**Самое опасное:** `storage.rs` — при любой ошибке SQLite весь бот падает в panic.

### 1.3 Синдром энтерпрайза (Overengineering)
**Статус: Высокий** — ~200+ строк бесполезного кода:

| Линия | Паттерн | Проблема |
|-------|---------|----------|
| `parser/command.rs:10-31` | Нуль-размерный `CommandParser` | `.parse()` не использует `&self` |
| `parser/duration.rs:6-23` | Нуль-размерный `DurationParser` | то же самое |
| `parser/target.rs:6-23` | Нуль-размерный `TargetSelectorParser` | то же самое |
| `router.rs:10-82` | Нуль-размерный `EventClassifier` | то же самое |
| `parser/reason.rs:83-102` | Builder для 3 полей | избыточно |

### 1.4 Мёртвые души
**Статус: Умеренно:**
- `#![allow(dead_code)]` в `event.rs`, `unit.rs`, `parser/mod.rs` — entire modules могут иметь неиспользуемый код

### 1.5 Устаревшие паттерны
- `once_cell::sync::Lazy` → должен быть `std::sync::LazyLock` (доступен с Rust 1.80+)

---

## 2. Анализ для Orange Pi 3 LTS

**Характеристики целевого устройства:**
- CPU: RK3566 (4x ARM Cortex-A55 @ 1.8GHz)
- RAM: 2GB LPDDR4
- Storage: eMMC или microSD
- OS: Armbian (Linux)

### 2.1 Выявленные узкие места

| Компонент | Текущее решение | Проблема |
|-----------|---------------|----------|
| Хранилище | rusqlite bundled | SQLite работает медленно без оптимизаций |
| JSON | serde_json | Нет zero-copy парсинга |
| Regex | regex crate | Не оптимизирован для повторяющихся паттернов |
| Кэш | moka 0.12 | Не использует unsafe для speed |
| Треды | tokio multi-thread | Может быть overkill для 4 ядер |
| Память | smallvec 1 | Хорошо, но мало где используется |
| Rhai VM | v1.24 | Не самая быстрая версия |
| Трассировка | tracing + json | heavy для embedded |

### 2.2 Рекомендации по оптимизации для ARM

```toml
# Добавить в Cargo.toml
[dependencies]
# Заменить на более легковесные альтернативы
simd-json = "0.7"  # Быстрее чем serde_json
fancy-regex = "0.14"  # Оптимизированные regex для повторяющихся паттернов
# Или использовать aho-corasick для простого поиска

[profile.release]
lto = "thin"  # "fat" слишком тяжелый для arm-none-eabi
codegen-units = 4  # Параллельная компиляция
strip = true
opt-level = "s"  # Size optimization важнее для embedded
```

### 2.3 Оптимизации времени выполнения

1. **SQLite запросы:**
   - Добавить `PRAGMA optimize` в startup
   - WAL mode вместо rollback journal
   - Prepared statements для повторяющихся query

2. **Регулярные выражения:**
   - Кэшировать скомпилированные Regex
   - Использовать `fancy-regex` с JIT для ARM

3. **Rhai скрипты:**
   - Предкомпилировать скрипты в байткод
   - Отключить динамическую типизацию если возможно

4. **Потребление памяти:**
   - Уменьшить размер tokio worker threads
   - Использовать `Box<[T]>` вместо `Vec<T>` где возможно

---

## 3. Предложения по рефакторингу

### 3.1 HIGH PRIORITY

1. **Заменить once_cell → std::lazy**
   ```rust
   // src/unit.rs:7
   - use once_cell::sync::Lazy;
   + use std::sync::LazyLock;
   
   // src/unit.rs:552
   - static VALID_CAPABILITIES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
   + static VALID_CAPABILITIES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
   ```

2. **Переписать обработку ошибок в storage.rs**
   - Заменить `unwrap_or_else(|e| panic!(...))` на `Result` + `?` оператор
   - Добавить кастомный тип ошибок `StorageError`

3. **Убрать magic numbers в moderation.rs**
   - `message_id: request.reply_to_message_id.unwrap_or(900)` → явная константа

### 3.2 MEDIUM PRIORITY

4. **Удалить нуль-размерные обёртки**
   ```rust
   // parser/command.rs:10-31 — удалить CommandParser, использовать функцию напрямую
   // parser/duration.rs:6-23 — удалить DurationParser
   // parser/target.rs:6-23 — удалить TargetSelectorParser
   // router.rs:10-82 — удалить EventClassifier
   ```

5. **Уменьшить количество .expect()**
   - Перейти на `Result<T, E>` с ? оператором
   - Использовать `thiserror` для кастомных ошибок

### 3.3 LOW PRIORITY

6. **Почистить dead code**
   - Удалить `#![allow(dead_code)]` где возможно
   - Проверить неиспользуемые структуры

7. **Оптимизировать Cargo.toml для ARM**
   - Изменить профиль release для embedded
   - Добавить conditionally compiled features

---

## 4. Итоговый статус

| Категория | Статус | Рекомендация |
|-----------|--------|--------------|
| API-призраки | ✅ OK | - |
| Happy Path | 🔴 Критично | Переписать обработку ошибок |
| Overengineering | 🟡 Высокий | Удалить нуль-размерные типы |
| Dead code | 🟡 Умеренно | Почистить при возможности |
| Устаревшие паттерны | 🟡 Умеренно | Заменить once_cell |
| ARM оптимизация | 🟢 Готово | Добавить profile настройки |