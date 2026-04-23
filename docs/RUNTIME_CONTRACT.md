# Runtime Contract

Этот документ описывает текущий runtime contract проекта. Он фиксирует только то, что уже подтверждается кодом.

## Config Loading

- `AppConfig::load()` сначала проверяет `TMO_CONFIG`.
- Если `TMO_CONFIG` задан, startup идет через fail-fast: файл должен существовать и парситься успешно.
- Если `TMO_CONFIG` не задан, runtime пытается прочитать локальный `config.toml`.
- Если `config.toml` отсутствует, приложение стартует с `AppConfig::default()`.

## Telegram Runtime

- `telegram.bot_token` опционален.
- `telegram.polling` по умолчанию `true`.
- Live polling ingress поднимается только если одновременно:
  - `telegram.polling = true`
  - задан `telegram.bot_token`
- Если `bot_token` не задан, Telegram transport остается `noop`, а polling loop не стартует.
- `telegram.admin_user_ids` используется для known-admin context в moderation и ingress.
- `telegram.primary_chat_ids` и `telegram.allowed_webhook_hosts` уже есть в конфиге, но текущий runtime path их не использует.

## Units Bootstrap

- Runtime грузит manifests из `paths.units_dir`.
- Если `paths.units_dir` не существует, runtime поднимается с пустым `UnitRegistry`.
- Если `paths.units_dir` существует, но это не директория, startup падает.
- Если в `paths.units_dir` есть невалидные manifests:
  - при `runtime.degraded_mode_enabled = true` runtime продолжает работу и сохраняет failed entries в registry summary
  - при `runtime.degraded_mode_enabled = false` `Runtime::from_config()` завершится ошибкой

## Storage And Fail-Fast

- На startup runtime bootstrap-ит storage schema и открывает отдельные storage handles для moderation, host API и ingress.
- Ошибки открытия или bootstrap storage являются fail-fast.
- Невалидные `storage.sqlite_journal_mode` и `storage.sqlite_synchronous` тоже валят startup при сборке runtime storage config.

## Dry-Run And Degraded Mode

- Runtime-wide dry-run toggle в конфиге сейчас нет.
- На startup `HostApi` создается с `dry_run = false`.
- Dry-run semantics существуют на уровне отдельных execution paths и команд, но не как глобальный runtime mode.
- `runtime.manual_mode_enabled` и `runtime.reload_enabled` уже есть в конфиге, но текущий startup path не меняет из-за них runtime behavior.
- `runtime.degraded_mode_enabled` сейчас влияет только на policy загрузки unit manifests.

## Observability

- `observability.log_level` применяется при инициализации `tracing`.
- `observability.json_logs` переключает формат между JSON и compact text.
- `RUST_LOG` тоже учитывается через `tracing_subscriber::EnvFilter`.
- `observability.metrics_enabled` и `observability.trace_sampling` уже есть в конфиге, но текущий `init_logging()` их не использует.

## Local Run

Локальный запуск из корня репозитория:

```bash
cargo run
```

С явным config path:

```bash
TMO_CONFIG=./config.toml cargo run
```

## Minimal Startup Checklist

Для live polling runtime достаточно:

- валидный `telegram.bot_token`
- `telegram.polling = true`
- доступный `paths.database_path`
- при использовании units: корректный `paths.units_dir`

Без `bot_token` приложение все равно может стартовать, но останется в degraded local runtime without live Telegram ingress.
