# Roadmap To MVP

## Цель

Довести текущий код до рабочего MVP moderation runtime, который:

- принимает реальные Telegram updates
- нормализует их в `EventContext`
- быстро классифицирует событие
- маршрутизирует его через indexed/bucketed router
- исполняет built-in moderation или unit-driven path без плоского перебора всех handlers
- пишет audit trail
- безопасно переживает replay/retry/dry-run

Ниже порядок работ после фиксации новой routing model.

## Целевая routing model для MVP

MVP должен строиться уже не вокруг плоского router, а вокруг indexed dispatch.

Базовая схема:

1. startup/reload
2. build handler indexes
3. ingress update
4. normalize to `EventContext`
5. classify event
6. produce `dispatch set`
7. run only relevant buckets
8. execute built-in или unit-driven lane

Важно:

это не модель “событие попадает ровно в одну корзину”.

Правильная модель:

- одно событие может иметь несколько traits
- router строит `dispatch set`, а не выбирает единственный bucket

Пример:

- text message
- reply
- command index hit: `warn`

Это уже не одна корзина, а набор релевантных индексов.

В этой модели важно не смешивать разные оси:

- ingress classes: `realtime | recovery | scheduled | manual`
- update traits: `text | callback | photo | voice | job | reply`
- command index: `warn | mute | ban | del | undo | msg`

## MVP definition

MVP можно считать достигнутым, когда одновременно выполнены все условия:

- runtime поднимается из `main`
- есть реальный Telegram ingestion loop
- есть startup/reload build routing indexes
- есть runtime-level event classification
- есть `dispatch set -> execution lane` path
- `/warn`, `/mute`, `/del`, `/undo` работают на живом transport
- audit и replay safety работают не только в модульных тестах
- dry-run остается предсказуемым

Текущий кодовый scope built-in moderation при этом уже шире MVP-gate:

- parser/executor уже знает `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`
- для MVP достаточно считать обязательным live coverage сначала для `/warn`, `/mute`, `/del`, `/undo`
- `/ban` и `/msg` остаются частью текущего built-in scope, но не обязаны быть главным release gate первого MVP

## Step 1. Дозавершить composition root

`Application` уже остается lifecycle shell, а `Runtime` уже существует как execution graph. Для MVP нужно довести этот composition root до unit-aware runtime.

Нужно сделать:

- собрать в одном месте `storage`, `unit_registry`, `telegram_gateway`, `router`, executors
- не принимать routing-решения в `Application`
- оставить `Application` lifecycle-контейнером
- добавить bootstrap units из `config.paths.units_dir` вместо пустого registry

Артефакт результата:

- один понятный runtime entrypoint

## Step 2. Ввести event classification и bucket indexes

Это главный архитектурный шаг.

Нужно сделать:

- определить event traits, по которым строятся indexes
- разделить command index, update trait indexes и ingress class indexes
- собирать indexes на startup
- пересобирать их на reload

Минимальный набор bucket groups:

- ingress class:
  - realtime
  - recovery
  - scheduled
  - manual
- update traits:
  - text
  - callback
  - photo
  - voice
  - job
- command index:
  - `warn`
  - `mute`
  - `ban`
  - `del`
  - `undo`
  - `msg`
- execution lane:
  - built-in moderation
  - unit-driven path

Полезная расшифровка ingress classes для MVP:

- `realtime` = live Telegram update
- `recovery` = replay Telegram update
- `scheduled` = scheduler/job input
- `manual` = synthetic manual invoke

Артефакт результата:

- runtime умеет делать `event -> dispatch set`, а не глобальный scan по всем units

## Step 3. Дозавершить реальный Telegram transport

`src/tg.rs` уже дает typed contract, а live transport уже подключается через `teloxide-core`, когда задан `bot_token`.

Нужно сделать:

- mapping `TelegramRequest -> Telegram API call`
- mapping response/error в `TelegramResult` и typed transport errors
- сохранить `noop` fallback только для конфигурации без токена

Минимальный MVP coverage:

- `tg.send_message`
- `tg.delete`
- `tg.delete_many`
- `tg.restrict`
- `tg.unrestrict`
- `tg.ban`
- `tg.unban`

## Step 4. Дозавершить ingress pipeline

Живой polling path уже есть. Для MVP нужно закрыть remaining gaps:

- raw Telegram update
- `TelegramUpdateInput`
- `EventContext`
- journaling
- dedupe/replay guard
- event classification
- router dispatch

Важно:

- journaling и dedupe должны жить в ingress/runtime path, а не быть случайным эффектом отдельных executors
- destructive guards должны срабатывать до исполнения side effects
- нужно добрать live ingress context, который сейчас теряется для admin/topic и части update types

## Step 5. Подвязать unit bootstrap и route envelopes

Сейчас unit schema и registry уже есть, но runtime еще не использует их как routing envelope.

Нужно сделать:

- загрузку manifests из `config.paths.units_dir`
- startup registry bootstrap
- reload strategy
- binding `event -> unit/route envelope`
- участие capabilities/config в execution route

Для MVP достаточно:

- command-trigger units
- media/update buckets для routing индексов
- built-in moderation как основная execution lane

Полный Rhai runtime можно пока не тащить.

## Step 6. Определить явную границу built-in vs scriptable

Важно не смешать два мира слишком рано:

- built-in moderation path
- unit-driven/scriptable path

Практический вариант для MVP:

- built-in lane остается текущим local executor path через `ModerationEngine`
- `/warn`, `/mute`, `/del`, `/undo` остаются MVP-critical built-in commands
- `/ban` и `/msg` остаются в текущем built-in scope и не должны выпадать из документации
- manifests участвуют как routing/policy/config envelope
- scriptable execution подключается после того, как собран runtime router

## Step 7. Доделать undo semantics

Перед MVP нужно усилить `/undo`.

Нужно сделать:

- более точный поиск исходного action
- понятную policy-модель для `undo`
- audit неуспешных undo попыток
- единое поведение для duplicate compensation

Часть проблем уже исправлена, но production-semantic hardening еще не закончен.

## Step 8. Закрыть config/runtime operations

Для MVP нужен понятный operational contract.

Нужно сделать:

- documented sample config
- Telegram token/config
- polling/webhook config
- unit manifests path
- dry-run toggle
- observability config
- fail-fast semantics для критичных runtime paths

Часть fail-fast уже есть, но документированный runtime contract еще не завершен.

## Step 9. Добавить e2e и integration tests

Текущие модульные тесты сильные, но не заменяют runtime scenarios.

Нужно добавить:

- ingress -> normalize -> classify -> route -> execute
- journal + audit assertions
- duplicate update replay scenario
- dry-run e2e
- undo e2e
- startup/reload index build tests
- routing bucket selection tests

Лучше делать это поверх test transport и temporary sqlite.

## Step 10. Подготовить операционную базу

Для первого MVP-релиза нужно:

- `.env`/config example
- команда локального запуска
- команда контейнерного запуска
- README с текущей execution model
- схема runtime lifecycle
- короткая схема routing buckets

## Предлагаемый порядок

1. `Runtime` composition root
2. event classification model
3. bucket/index build
4. реальный Telegram transport
5. ingress pipeline
6. unit bootstrap и route envelopes
7. e2e на built-in moderation path
8. потом scriptable path

## Что можно отложить после MVP

- hot reload manifests в полном объеме
- полноценный Rhai sandbox/runtime
- webhook deployment mode
- advanced retry scheduler
- semantic/vector features
- сложные UI flows и inline templates
- расширенный recovery orchestration

## Что нельзя откладывать

- runtime-level indexed routing
- реальный Telegram transport
- dedupe/replay safety
- audit correctness
- runtime config contract
- end-to-end happy path tests
- capability enforcement

## Практический короткий маршрут

Если идти максимально прагматично, маршрут такой:

1. собрать `Application -> Runtime`
2. ввести `classify -> dispatch set`
3. поднять real Telegram loop
4. встроить journal/dedupe в ingress
5. подключить built-in moderation execution через router
6. подтянуть unit envelopes
7. только потом решать, сколько scriptable execution влезает в MVP

Это самый короткий путь к живому runtime без возврата к плоскому перебору handlers.
