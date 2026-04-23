# ML Server Contract

Этот документ фиксирует текущий contract layer для интеграции с локальным `ml-server`, описанным в [ML-serer/README.md](/home/arch/Документы/Teloxide/ML-serer/README.md:1).

Важно:

- это уже typed contract в `HostApi`
- runtime transport wired только для `MlHealth` и `MlModels`
- `MlEmbedText` и `MlChatCompletions` пока остаются on the existing unavailable path

## Что уже есть

В `HostApi` добавлены typed operations:

- `MlHealth`
- `MlEmbedText`
- `MlChatCompletions`
- `MlModels`

Они живут рядом с остальными host contracts в [src/host_api.rs](/home/arch/Документы/Teloxide/src/host_api.rs:1).

## Capability Model

Для этих вызовов используются explicit capabilities:

- `ml.health.read`
- `ml.embed_text`
- `ml.chat`
- `ml.models.read`

Allow-list валиден на уровне unit schema: [src/unit.rs](/home/arch/Документы/Teloxide/src/unit.rs:1).

## Request Surface

Текущие typed requests:

- `MlHealthRequest { base_url: Option<String> }`
- `MlEmbedTextRequest { base_url: Option<String>, input: Vec<String>, model: Option<String> }`
- `MlChatCompletionsRequest { base_url: Option<String>, model: String, messages: Vec<MlChatMessage>, max_tokens: Option<u32> }`
- `MlModelsRequest { base_url: Option<String> }`

Для chat используется:

- `MlChatMessage { role: String, content: String }`

`base_url` опционален специально:

- можно оставить `None`, если runtime должен взять `ml_server.base_url` из config
- можно передавать конкретный URL явно, если caller хочет полный control contract уже сейчас

## Current Semantics

Сейчас поведение намеренно консервативное:

- request проходит через `EventContext` validation
- request проходит через capability gate
- request проходит через базовую field validation
- при `HostApi::dry_run() == true` health/models возвращают typed planning response
- при обычном режиме health/models делают real HTTP call to resolved `base_url`
- embed/chat still завершаются structured `ResourceUnavailable` с ресурсом `ml_server_transport`

Это означает:

- contract уже можно использовать в unit/plugin codegen и tests
- runtime теперь делает real HTTP calls for the first useful slice, without pretending the whole ML surface is done

## Planning Responses

Dry-run path сейчас возвращает planning metadata вместо fake ML output.

Примеры:

- `MlHealthValue { base_url, resolved_base_url, status: None, provider: None, model: None, transport_ready: false }`
- `MlEmbedTextValue { base_url, model, input_count, transport_ready: false }`
- `MlChatCompletionsValue { base_url, model, message_count, max_tokens, transport_ready: false }`
- `MlModelsValue { base_url, resolved_base_url, models: [], transport_ready: false }`

То есть dry-run уже дает удобный typed envelope, но не подделывает embeddings, models list или LLM answer.

## Live Values

В live path `MlHealthValue` теперь carries:

- `status`
- `provider`
- `model`

В live path `MlModelsValue` теперь carries typed model summaries:

- `id`
- `object`
- `owned_by`
- `created`

Это минимальный полезный mapping на текущий `ml-server` response surface.

## Practical Use

Этот слой уже полезен для двух задач:

1. проектировать plugins/units под нормальные `HostApiRequest`, а не под ad-hoc `sys.http.fetch`
2. заранее зафиксировать capability envelope для ML units

Практическое правило:

- embeddings/plugin под vector lookup: `ml.embed_text`
- LLM chat/plugin: `ml.chat`
- health/model inspection plugins: `ml.health.read`, `ml.models.read`

## Not Yet Implemented

Пока еще нет:

- replay/dry-run policy поверх реального ML transport

Это уже следующий шаг после нынешнего contract slice.
