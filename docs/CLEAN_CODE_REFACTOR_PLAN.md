# Clean Code Refactor Plan

## Цель

Описать рефактор, который:

- не ломает текущий working moderation core
- не возвращает проект к плоскому router scan
- собирает runtime-level indexed router
- разделяет classification, routing и execution

Документ исходит из новой целевой модели:

- не плоский роутер
- не длинная цепочка `if/else`
- а `classify -> dispatch set -> execution lane`

## Короткий диагноз

Проблема текущего кода не в том, что он уже стал хаотичным.

Проблема в другом:

- хорошие локальные слои уже есть
- runtime-level router еще отсутствует
- если продолжать без явного routing layer, логика начнет расползаться по `app.rs`, `moderation.rs`, `host_api.rs`

То есть следующий риск — не parser mess, а несколько параллельных оркестраторов.

## Что уже выглядит хорошо и не требует переписывания

Эти части надо сохранять:

- `EventContext` и его инварианты
- `EventNormalizer`
- parser chain
- `EventCommandDispatcher`
- typed `TelegramRequest` / `TelegramResult`
- tg dry-run/idempotency shell
- storage contracts вокруг `audit_log`, `jobs`, `processed_updates`
- built-in moderation semantics `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`

Ключевая мысль:

рефактор нужен вокруг runtime routing и orchestration,
а не внутри parser AST и moderation semantics.

## Новый архитектурный принцип

Новый router должен быть не плоским scan router, а indexed router.

Это значит:

- startup/reload строит индексы
- runtime event сначала классифицируется
- router получает `dispatch set`
- исполняются только релевантные buckets

Нельзя проектировать router как:

- “одна корзина на событие”
- “перебираем все units, пока что-то подойдет”

Правильная модель:

- одно событие может иметь несколько traits
- router оперирует не одним bucket, а набором индексов и execution lanes

## Где код уже просит рефактор

## 1. `Application` не должен быть router

Сейчас `Application` правильно остается lifecycle shell, но вокруг него нужен отдельный runtime graph.

Нужно:

- оставить lifecycle в `app.rs`
- вынести execution wiring в `Runtime`

## 2. Нет отдельного classification слоя

Сейчас вход сразу прыгает в локальные execution paths.

Нужно:

- отделить определение event traits от исполнения

Без этого bucketed router не получится.

## 3. Нет prebuilt indexes

Сейчас есть parser и manifests, но нет runtime structure вида:

- `command_index`
- `update_trait_index`
- `ingress_class_index`

Нужно:

- строить эти индексы на startup/reload

## 4. `ModerationEngine` слишком широкий

Он все еще совмещает:

- command dispatch
- часть capability checks
- tg execution
- storage writes
- audit orchestration
- job scheduling

Он должен остаться built-in executor'ом,
но перестать быть верхним router.

## 5. Capability enforcement все еще не выглядит единым policy layer

Часть логики уже зачищена, но архитектурно нужен отдельный evaluator, чтобы:

- built-in path
- Host API
- будущий unit-driven path

ходили через одну модель policy.

## Целевая структура

Ниже рекомендуемая структура по ответственностям.

## `src/runtime.rs`

Ответственность:

- собрать runtime graph
- владеть startup-time indexes
- держать ingress/router/executor wiring

Держит:

- `Storage`
- `UnitRegistry`
- `TelegramGateway`
- `HostApi`
- `IngressPipeline`
- `ExecutionRouter`
- built-in executors
- routing indexes

## `src/app.rs`

Ответственность:

- lifecycle
- startup/shutdown
- делегирование в `Runtime`

Не должен:

- принимать routing decisions
- знать про buckets

## `src/ingress/mod.rs`

Ответственность:

- общий ingress pipeline

## `src/ingress/telegram.rs`

Ответственность:

- polling/webhook updates
- convert raw update в `TelegramUpdateInput`

## `src/ingress/pipeline.rs`

Ответственность:

единый путь:

1. принять внешний input
2. normalizer -> `EventContext`
3. journaling
4. dedupe/replay guard
5. classification
6. router dispatch

Важно:

journaling и dedupe должны быть ingress concern.

## `src/classifier.rs`

Ответственность:

- выделить traits события
- построить runtime `EventTraits`

Например:

- ingress class
- message/media kind
- command presence
- callback presence
- reply trait

Это отдельный слой, потому что classification не должен смешиваться с execution logic.

## `src/router.rs`

Ответственность:

- принять `EventContext` + `EventTraits`
- вычислить `dispatch set`
- выбрать execution lanes

Целевой интерфейс:

```rust
pub struct ExecutionRouter { ... }

impl ExecutionRouter {
    pub async fn route(&self, event: EventContext) -> Result<ExecutionOutcome, ExecutionError>;
}
```

Важно:

router не должен сам выполнять бизнес-операции.

Он должен:

- выбирать buckets
- выбирать executor
- оркестрировать порядок lane execution

## `src/router_index.rs`

Ответственность:

- хранить prebuilt indexes
- пересобираться на startup/reload

Минимальные индексы:

- `HashMap<command_name, handlers>`
- `HashMap<update_trait, handlers>`
- `HashMap<ingress_class, handlers>`

При этом command — отдельный индекс поверх text/callback/manual/scheduled command source, а не отдельный transport update type.

Полезная фиксация терминов:

- ingress classes: `realtime | recovery | scheduled | manual`
- update traits: `text | callback | photo | voice | job | reply`
- command index: `warn | mute | ban | del | undo | msg`

## `src/unit_binding.rs`

Ответственность:

- `event -> unit/route envelope`
- binding manifest-aware execution metadata
- отделение routing knowledge от manifest lookup

## `src/policy.rs`

Ответственность:

- единый capability evaluator
- allow/deny decisions
- policy envelope

Этим слоем должны пользоваться:

- `HostApi`
- built-in moderation execution
- будущий scriptable path

## `src/builtin/moderation.rs`

Ответственность:

- built-in moderation executor

По текущему коду это прямой наследник нынешнего `ModerationEngine`-path: локальный built-in executor, но не top-level runtime router.

Что оставить внутри:

- бизнес-семантику `/warn`, `/mute`, `/ban`, `/del`, `/undo`, `/msg`

Что вынести:

- общий routing
- classification
- ingress orchestration
- unit binding
- общий policy layer

## Как резать текущий код

## Step 1. Вынести `Runtime`

Первый шаг должен быть почти механическим:

- отделить runtime graph от `Application`
- не менять execution behavior

Цель:

получить один composition root.

## Step 2. Ввести `Classifier`

На этом шаге:

- появляется отдельный слой `EventContext -> EventTraits`
- пока без сложной multi-lane orchestration

Цель:

отделить routing classification от business execution.

## Step 3. Ввести `RouterIndex`

На этом шаге:

- startup/reload начинают собирать indexes
- built-in moderation descriptors тоже становятся частью index model

Цель:

уйти от будущего плоского перебора handlers.

## Step 4. Ввести `ExecutionRouter`

На этом шаге:

- `router.route(event)` пока может делегировать только в built-in moderation lane
- но уже через `dispatch set`

Цель:

сделать одну точку входа для routing решений.

## Step 5. Перенести journaling и dedupe в ingress pipeline

На этом шаге:

- верхнеуровневые destructive guards живут в ingress
- built-in executor перестает быть носителем ingress semantics

Цель:

держать replay safety в одном месте.

## Step 6. Выделить `UnitBinder`

На этом шаге:

- realtime events получают route envelope
- `event -> unit -> execution lane` становится явным

Цель:

подготовить manifest-aware routing без полноценного Rhai runtime.

## Step 7. Вынести `PolicyEvaluator`

На этом шаге:

- `HostApi`
- built-in executors
- будущий scriptable path

используют один evaluator.

Цель:

убрать дубли capability logic.

## Step 8. Уменьшить `ModerationEngine`

После предыдущих шагов можно безопасно уменьшать его объем:

- оставить built-in semantics
- убрать из него роль верхнего router/orchestrator

Цель:

сделать его focused executor'ом.

## Что нельзя трогать без отдельной миграции

High-risk зоны:

- shape `EventContext`
- parser AST types
- `processed_updates` semantics
- `audit_log` meaning
- `TelegramRequest` / `TelegramResult` contract
- текущую undo/reverse модель без точных тестов

Рефактор runtime graph не должен одновременно менять эти контракты.

## Признаки хорошего результата

Рефактор можно считать удачным, если после него:

1. есть один runtime entrypoint
2. есть отдельный `Classifier`
3. есть startup/reload routing indexes
4. `ExecutionRouter` выбирает `dispatch set`, а не сканирует все подряд
5. `Application` не принимает routing decisions
6. `ModerationEngine` отвечает только за built-in moderation semantics
7. policy живет в одном месте
8. ingress отвечает за journal/dedupe

## Признаки плохого рефактора

Нужно тормозить работу, если появляется один симптомов:

- routing rules дублируются в `app.rs`, `moderation.rs`, `host_api.rs`
- `ExecutionRouter` становится новым god-object
- classification смешивается с execution logic
- bucket model деградирует обратно в глобальный scan
- built-in и scriptable path смешиваются раньше, чем собран runtime base

## Самый прагматичный маршрут

Если идти коротким и безопасным путем:

1. `Runtime`
2. `Classifier`
3. `RouterIndex`
4. `ExecutionRouter`
5. `IngressPipeline`
6. `UnitBinder`
7. `PolicyEvaluator`
8. уменьшение `ModerationEngine`

Это дает чистый код и готовит проект именно к indexed router, а не к еще одному fat orchestrator.
