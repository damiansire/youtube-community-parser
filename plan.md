# Plan — Ecosistema para creadores (ideas de video + SEO + benchmark)

## Context

Hoy `Subscriptor-Data-Parser` es estrictamente **análisis de comunidad**: ingesta
comentarios (YouTube Data API v3) y rankea quién comenta más/menos
(`sdp-core::rank_commenters`), persiste el histórico en SQLite (`sdp-storage`) y
respeta topes de cuota con resultados parciales (F4). El proyecto va por **F5**.

El pedido es convertirlo en un **ecosistema que ayude a los creadores**: ideas de
video, sugerencias SEO, keywords/temas y benchmark de competidores. Decisiones ya
tomadas con el usuario:

1. **Motor híbrido por fases**: primero un motor **heurístico en Rust puro**
   (determinista, testeable, sin costo), con punto de extensión para enchufar una
   **IA multi-proveedor (Anthropic *o* Gemini)** como capa **opcional** después.
   El norte es **acompañar al creador en todo su proceso de creación de video**
   (idear → estrategia → SEO/packaging → refinar con IA).
2. **Las 4 familias de features** entran al roadmap (ideas, SEO, keywords,
   benchmark).
3. **Ingesta ampliada** a metadata de videos (`videos`, barato) y
   `search`/trending (caro).
4. **Gating de costos obligatorio**: toda operación cara (cuota alta tipo
   `search` = 100u, o llamadas a IA que cuestan dinero) es **opt-in**: el
   usuario hace click y la UI le muestra el **costo estimado antes** de ejecutar.

Esto es un **roadmap**: describe las fases F6→F12, una por una, cada una un diff
chico y revisable.

## Principio transversal — el gate de costo

Se diseña chico en **F6** (donde el costo siempre es 0) para tenerlo listo cuando
llegue lo caro. Contrato: **estimar → confirmar → ejecutar**.

- **Dominio puro** en `sdp-core` (módulo nuevo `cost`):
  - `enum CostKind { QuotaUnits(u32), Money { usd_micros: u64 } }` — separa cuota
    de YouTube (entera, finita) de dinero real (IA); `usd_micros` evita floats.
  - `struct CostEstimate { kind, breakdown: Vec<CostLine>, requires_confirmation: bool }`
    para que la UI muestre el desglose ("search.list: 100u", "videos.list: 2u").
  - Funciones **puras** `estimate_*(params) -> CostEstimate` por operación, según
    la tabla de cuota v3 (search=100, videos=1, commentThreads=1, playlistItems=1).
  - `fn needs_optin(estimate, policy) -> bool` (todo `Money` y todo `QuotaUnits`
    sobre umbral configurable → opt-in).
- **Mecánica IPC** en `lib.rs` (sin lógica de negocio): cada operación cara se
  parte en **dos comandos** — `estimate_<op>(params) -> CostEstimate` (gratis,
  solo calcula) y `run_<op>(params, confirmed) -> Resultado` (solo tras
  confirmación; **re-calcula el estimate server-side** y no confía en el front).
- **UI** (`app/main.js`): helper `confirmCost(estimate)` que muestra un modal
  ("~100 unidades / ~US$0,012 — ¿continuar?") y solo entonces llama al `run_*`.
  Operaciones gratis (F6–F8) muestran "0 / gratis" y pueden auto-confirmar.

## Roadmap por fases (menor → mayor costo/riesgo)

### F6 — Keywords y temas recurrentes (heurístico, costo 0)
Cimiento del motor: mina el corpus **ya persistido**, sin red.
- **`sdp-core`** (módulo `text`/`corpus`): `KeywordStats`, `Topic`,
  `CorpusInsights`; funciones puras `normalize_text`, `stopwords_es_en` (lista
  embebida ES+EN), `extract_keywords` (frecuencia + peso tipo tf-idf,
  desempate determinista igual que `rank_commenters`), `cluster_topics`
  (co-ocurrencia / n-gramas, **determinista**, nada de ML).
- **`lib.rs`**: comando `analyze_corpus(app) -> CorpusInsights` (sobre histórico).
- **UI**: sección "Temas de la comunidad" (lista/nube de keywords + temas).
  Escapar términos remotos con el `escapeHtml` ya existente.
- **Tests** (core): tokenización, stopwords, término dominante, tf-idf baja
  ubicuos, determinismo, corpus vacío → vacío. Fixtures inline (estilo `ranking.rs`).

### F7 — Ideas de video desde la comunidad (heurístico, costo 0)
Reusa F6. Mayor valor percibido; arranque pedido.
- **`sdp-core`** (módulo `ideas`): `enum DemandSignal { Question, Request, RecurringTopic }`,
  `VideoIdea { title_seed, signal, supporting_comment_ids, score, sample_quotes }`;
  `detect_questions`/`detect_requests` (heurísticos ES+EN), `mine_video_ideas`
  (combina señales + temas de F6 + puntúa por frecuencia/likes; determinista).
- **`lib.rs`**: `mine_ideas(app) -> Vec<VideoIdea>`.
- **UI**: cards de ideas (semilla, badge de señal, score, citas escapadas). Botón
  "Refinar con IA" **placeholder deshabilitado** (se activa en F12).
- **Tests**: detectores (positivos/negativos, ES+EN), agrupado por tema, score
  respeta likes/frecuencia, determinismo, sin señales → vacío.

### F8 — SEO heurístico de texto candidato (costo 0)
Capa heurística de SEO sobre título/tags/descripción que **pega el usuario** (sin
red todavía; la metadata real llega en F9).
- **`sdp-core`** (módulo `seo`): `SeoInput`, `SeoSeverity`, `SeoFinding`,
  `SeoReport { score: u8, findings, missing_community_keywords }`;
  `audit_title`/`audit_tags`/`audit_description` (largos recomendados, stuffing,
  tags vacíos/duplicados) y `audit_seo(input, corpus)` que cruza con las keywords
  demandadas por la comunidad (F6).
- **`lib.rs`**: `audit_seo(app, input) -> SeoReport`.
- **UI**: formulario + reporte por severidad con score. Botón "Sugerir con IA"
  placeholder (F12).
- **Tests**: por regla (título corto/largo, tags vacíos/dup, keyword faltante),
  score, determinismo.

### F9 — Ingesta de metadata de videos (`videos.list`, 1u) + estrena el gate real
Primera operación de red nueva; barata pero ya pasa por `confirmCost`.
- **`sdp-core`**: `VideoMeta { video_id, channel_id, title, description, tags,
  view_count, like_count, comment_count, published_at }` (dato de dominio puro).
- **`youtube.rs`**: structs serde de `videos.list`, mapeo **puro**
  `video_item_to_meta` (testeable con fixture JSON), método
  `fetch_video_meta(ids) -> Result<Vec<VideoMeta>>` (chunks de 50, paginado;
  sin cache/persistencia acá — boundary).
- **`sdp-storage`**: tabla `video_meta` (PK `video_id`), upsert idempotente con el
  mismo patrón que `comment`; tags como JSON en columna TEXT. `save_video_meta`,
  `all_video_meta`.
- **`lib.rs`**: `estimate_video_meta(ids) -> CostEstimate` (= ceil(n/50) unidades)
  y `fetch_video_meta(app, ids, api_key, confirmed)` (persiste vía
  `spawn_blocking`, igual que `persist`); más `audit_seo_for_video(app, video_id)`
  que arma `SeoInput` desde el `VideoMeta` guardado.
- **UI**: "Traer metadata de mis videos" con `confirmCost` (estrena el modal,
  muestra ~Nu); SEO precarga título/tags reales.
- **Tests**: core (round-trip + mapeo con fixture, `estimate_video_meta`),
  storage (round-trip + idempotencia in-memory), youtube.rs (chunking con
  `wiremock`, como los `http_tests` actuales).

### F10 — Búsqueda / trending (`search.list`, 100u) detrás del gate
Cara: opt-in estricto, modal prominente, topes agresivos por defecto.
- **`sdp-core`**: `SearchHit { video_id, channel_id, title, published_at }`,
  `SearchPlan { query, trending, max_pages }`; `estimate_search` (= 100 ×
  `max_pages`, `requires_confirmation = true` siempre).
- **`youtube.rs`**: serde de `search.list`, mapeo puro `search_item_to_hit`,
  `search(plan)`; reusa la resiliencia de cuota existente (`is_quota_exceeded` →
  parcial con `incomplete`).
- **`sdp-storage`** (opcional): tabla `search_cache` (query+fecha → hits) para no
  repagar 100u al reabrir; idempotente (mantiene el principio "no volver a gastar
  cuota").
- **`lib.rs`**: `estimate_search(plan)` / `run_search(app, plan, api_key, confirmed)`.
- **UI**: buscador con aviso destacado ("100 unidades por página"); `confirmCost`
  **sin** auto-confirm; default `max_pages = 1`.
- **Tests**: core (`estimate_search`), youtube.rs (fixture + paginación/quota con
  wiremock), storage (cache round-trip si se implementa).

### F11 — Benchmark de competidores (heurístico; compone F6/F9/F10)
No agrega endpoints: orquesta metadata (F9) y opcionalmente descubrimiento (F10).
- **`sdp-core`** (módulo `benchmark`): `ChannelProfile { ..., avg_views,
  avg_likes, avg_comments, top_keywords, posting_cadence_days }`,
  `BenchmarkReport { mine, competitors, gaps }`, `BenchmarkGap`;
  `profile_channel(videos, comments)` (reusa `extract_keywords` de F6) y
  `benchmark(mine, competitors)` (gaps deterministas: keywords que ellos cubren y
  yo no, cadencia, engagement).
- **`lib.rs`**: `benchmark_channels(app, my_id, competitor_ids) -> BenchmarkReport`
  sobre datos ya ingestados; si falta un competidor, devuelve gap "sin datos" en
  vez de fallar (estilo `incomplete`).
- **UI**: tabla comparativa + gaps accionables (reusa patrones de tabla/roster).
- **Tests**: `profile_channel` (promedios, cadencia), `benchmark` (gaps, empate,
  competidor vacío), determinismo.

### F12 — Capa IA opcional multi-proveedor (Anthropic *o* Gemini) detrás de un trait + gate de dinero
El punto de extensión prometido: asiste al creador en su proceso (refinar ideas,
reescribir SEO, etiquetar temas). El heurístico sigue siendo el default gratis; la
IA es opt-in con costo en US$. **El proveedor es elegible**: Anthropic o Gemini,
intercambiables tras el mismo contrato.
- **Dominio puro intacto**: en `sdp-core` solo el **contrato puro** — prompt y
  parseo son **funciones puras testeables** (`build_ideas_prompt(...) -> EnhancePrompt`,
  `parse_ideas_response(raw) -> Result<Vec<VideoIdea>, ParseError>`). El core
  **no** conoce async/red ni ningún proveedor. El prompt se construye una vez y
  sirve para cualquier modelo; el parseo tolera variaciones de formato.
- **Crate nuevo `crates/llm` (`sdp-llm`)**: define un **trait de proveedor**
  (`InsightProvider`, async con `reqwest`) y **dos adaptadores intercambiables**:
  `AnthropicProvider` y `GeminiProvider`. Un `enum Provider { Anthropic, Gemini }`
  selecciona en runtime. El contrato multi-proveedor, el retry header-aware y el
  parseo robusto siguen la skill **`genai-app-patterns`**; para los detalles de
  Anthropic (model IDs/pricing) consultar la skill **`claude-api`** (modelo más
  capaz vigente, p. ej. Opus 4.8 `claude-opus-4-8`); para Gemini, su API REST
  (p. ej. `gemini-2.x`) y su tabla de pricing. La key del proveedor elegido en
  `secrecy::SecretString`, **igual** que la de YouTube (no se persiste, se pide por
  sesión → reusa el modelo de amenaza ya documentado en el README).
- **Costo en dinero**: `estimate_*_ai(provider, ...) -> CostEstimate` con
  `CostKind::Money` (tokens estimados × pricing **del proveedor/modelo elegido**);
  `requires_confirmation = true` siempre.
- **`lib.rs`**: por feature, par `estimate_<f>_ai` / `run_<f>_ai(app, provider,
  params, api_key, confirmed)`; inyecta el adaptador concreto según `provider`
  solo si el usuario opta por IA. Salidas reusan `VideoIdea`/`SeoReport`/`Topic`
  con `source: { Heuristic | Ai { provider } }`.
- **UI**: se activan los botones "Refinar/Sugerir con IA" (placeholders de F7/F8):
  el usuario **elige proveedor** (Anthropic/Gemini), pega su key (input password),
  ve `confirmCost` en US$, y recién ahí se llama al `run_*_ai`.
- **Tests**: core puro sin red/key (`build_*_prompt` snapshot, `parse_*_response`
  con fixtures incl. malformadas → `ParseError`, `estimate_*_ai`); `sdp-llm`
  contra `wiremock` (ambos adaptadores: Anthropic y Gemini) + un test `#[ignore]`
  real por proveedor con key del entorno (mismo patrón que el `verifica_fetch_real`
  actual).

## Resumen de orden, costo y riesgo

| Fase | Feature | Costo | Red nueva | Riesgo |
|------|---------|-------|-----------|--------|
| F6 | Keywords/temas | 0 | no | bajo |
| F7 | Ideas de video | 0 | no | bajo |
| F8 | SEO heurístico | 0 | no | bajo |
| F9 | Metadata `videos` + gate real | 1u/50 | sí | bajo-medio |
| F10 | `search`/trending | 100u | sí | medio-alto |
| F11 | Benchmark competidores | reusa F9/F10 | no | medio |
| F12 | Capa IA opcional (Anthropic/Gemini) | US$ | sí (Anthropic o Gemini) | alto |

## Qué va a poder hacer el creador (capacidades por etapa)

1. **Idear** — ideas de video desde su propia comunidad (pedidos/preguntas/temas
   con citas que las respaldan); mapa de temas y keywords; descubrimiento por
   búsqueda/trending.
2. **Estrategia** — benchmark vs competidores (engagement, cadencia, keywords que
   cubren ellos y él no); detección de demanda no cubierta.
3. **SEO / packaging** — auditoría de título/tags/descripción con score 0–100;
   SEO sobre sus videos reales (metadata); sugerencias de títulos/tags/descripción
   con IA.
4. **Refinar con IA** — ideas pulidas (gancho/ángulo), reescritura SEO, etiquetado
   de temas. Siempre opcional; el heurístico funciona gratis.
5. **Sin sorpresas de costo** — gate con estimación antes de cada operación cara;
   histórico local sin regastar cuota; elección de proveedor de IA con su key.

## Archivos críticos
- `crates/core/src/lib.rs` — reexports + módulos nuevos (`cost`, `text`, `ideas`,
  `seo`, `benchmark`, `insight`). **Acordarse de reexportar `VideoMeta` al tocar F9.**
- `crates/core/src/models.rs` / `ranking.rs` — patrón de structs serde + desempate
  determinista a imitar; `most_active/least_active` los usa `benches/ranking_bench.rs`
  (no romper su firma sin actualizar el bench).
- `src-tauri/src/lib.rs` — orquestación; pares `estimate_*` / `run_*` del gate
  (sigue el patrón de `analyze_*` + `persist` + `spawn_blocking`).
- `src-tauri/src/youtube.rs` — endpoints `videos`/`search`, mapeos **puros** con
  fixtures; reusa `IngestLimits`, `Ingested.incomplete` y `is_quota_exceeded`.
- `crates/storage/src/lib.rs` — tablas `video_meta` y `search_cache`, upsert
  idempotente (patrón `ON CONFLICT ... DO UPDATE` ya presente).
- `app/main.js` / `app/index.html` — modal `confirmCost`, secciones nuevas,
  inputs de keys; reusar `escapeHtml` para todo texto remoto.
- `crates/llm/` (nuevo, F12) — trait `InsightProvider` + adaptadores
  `AnthropicProvider` y `GeminiProvider` (intercambiables).

## Convenciones a respetar (del CLAUDE.md global y del repo)
- **Por fases chicas y revisables**: una fase = un diff acotado; parar a mostrar
  al terminar cada hito con build/tests limpios.
- **Tests de dominio ANTES que UI**; si se toca dominio, dejar test que lo cubra.
- **Boundaries**: `sdp-core` puro (sin red/UI/I-O), `sdp-storage` sin red/UI,
  `youtube.rs` sin cache/persistencia, `lib.rs` orquesta. Si aparece un import "al
  revés", parar y reubicar.
- **Commits**: conventional en español (`feat(core): …`, `feat(ingest): …`,
  `feat(storage): …`, `feat(llm): …`), **sin** atribución a Claude. Commit/push
  solo cuando se pida; **nunca push sábados/domingos**.
- **UI renderizada**: pasar el loop de `design-reviewer` antes de dar por terminada
  cada pantalla nueva (temas, ideas, SEO, benchmark).

## Verificación (por fase)
- **F6–F8, F11 (puro)**: `cargo test -p sdp-core` (no requiere MSVC). Cubrir el
  determinismo y los casos vacíos como hace `ranking.rs`.
- **F9–F10 (red + storage)**: `cargo test -p sdp-storage -p sdp-desktop`
  (requiere MSVC); HTTP mockeado con `wiremock`; test real `#[ignore]` con
  `YOUTUBE_KEY_API` del entorno cuando se quiera validar contra la API.
- **F12 (LLM)**: `cargo test -p sdp-llm` (wiremock) + `cargo test -p sdp-core`
  (prompts/parseo puros); test real `#[ignore]` por proveedor con key del entorno.
- **End-to-end**: `cargo tauri dev`, validar que cada operación cara muestra el
  modal de costo y **no** ejecuta sin confirmación explícita.

## Próximo paso sugerido
Arrancar por **F6** (keywords/temas) — costo 0, todo en `sdp-core` con tests, sin
tocar red ni UI primero. Es la base de F7 (ideas) y F8 (SEO).
