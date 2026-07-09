# rpi — адопция ручного cloudflared в `agent setup` + громкий manual-ingress (дизайн)

Дата: 2026-07-09. База: `2026-07-09-rpi-cloudflare-lan-automation-design.md` (Phase 1,
§5 bootstrap); закрывает разрывы, вскрытые инцидентом «rpi.iiskelo.com не получил
роут/DNS при деплое» на Pi с руками собранным туннелем.

**Критерий готовности.** На Pi с ручной cloudflared-инсталляцией (living config.yml
с ручными роутами, работающий user-юнит) одна команда
`sudo rpi agent setup --with-cloudflared --cf-token <tok> --domain <zone>`:

1. адоптирует туннель **без единой записи в существующий `config.yml` и без рестарта
   cloudflared** — ноль downtime для уже опубликованных hostname'ов;
2. включает деплой-тайм автоматику: каждый следующий `rpi deploy` проекта с
   `[ingress] hostname` сам вписывает роут, заводит CNAME и рестартует туннель;
3. а до включения — ситуация «hostname объявлен, ingress выключен» видна в итоговой
   сводке деплоя и в `rpi doctor`, не только строкой в середине лога.

---

## 1. Контекст и вскрытые разрывы (v0.13.0)

- `cloudflared_bootstrap_full` (`crates/bin/src/agent/setup.rs`) безусловно
  перезаписывает `config.yml` голым рендером (tunnel + credentials-file + catch-all
  404) — без `.bak` и без переноса существующих роутов. Это нарушает принцип
  Phase 1-плана «divergent managed files are backed up to `*.bak`» и стирает ручные
  роуты до следующего редеплоя каждого проекта.
- Без `--tunnel-name` имя туннеля деривится из hostname машины → на хосте с
  существующим туннелем под другим именем bootstrap **создаёт новый туннель** и
  переводит config.yml на него, пока DNS смотрит на старый.
- Проверка credentials требует канонический путь `/var/lib/rpi/cloudflared/<id>.json`
  и игнорирует `credentials-file:` из самого конфига.
- Дефолтная `restart`-команда `systemctl --user restart cloudflared` выполняется
  агентом без `XDG_RUNTIME_DIR` (юнит rpi-agent ставит только HOME/XDG_CONFIG/XDG_CACHE)
  — на живом хосте она падает, и `CloudflaredIngress` откатывает свежевписанный роут.
- `DisabledIngress` сообщает о ручном роутинге одной строкой в середине деплой-лога;
  ни итоговая сводка, ни `rpi doctor` о выключенном ingress не говорят.

## 2. Скоуп

**Входит:** адопция существующей инсталляции в `cloudflared_bootstrap_full`;
инъекция `XDG_RUNTIME_DIR` в restart-команду; warning в итоге деплоя + чек в
`rpi doctor`; README.

**НЕ входит (YAGNI):** канонизация/мердж существующего config.yml (роуты и дальше
точечно ведёт деплой-тайм `upsert_ingress_rule`); адопция конфигов вне канонического
пути `/var/lib/rpi/cloudflared/config.yml` (README подсказывает перенести); оформление
адопции как записи фреймворка `rpi agent migrate` (адопции нужны токен и домен —
это входы `setup`, у которого флаги уже есть); запуск setup с ноутбука без SSH.

## 3. Адопция в `cloudflared_bootstrap_full`

Новая развилка в начале функции по состоянию `config.yml` на каноническом пути.

### 3.1. Файл есть и парсится (mapping с ключом `tunnel:`) → адопция

1. **Резолв `tunnel_id`.** Значение `tunnel:` UUID-образное (36 символов,
   дефисы, hex) → это готовый id, **Tunnel-API не вызывается вовсе** (токену
   достаточно `Zone:Zone:Read + Zone:DNS:Edit`). Значение — имя → существующая
   adopt-ветка `find_or_create_tunnel` (пустой secret = адопция; требует
   `Account:Cloudflare Tunnel:Edit`).
2. **Credentials.** Путь берётся из `credentials-file:` самого конфига (fallback —
   канонический `/var/lib/rpi/cloudflared/<id>.json`, если ключа нет); файл должен
   существовать, иначе ошибка с actionable-сообщением. Ничего не перезаписываем.
3. **`config.yml` не записывается. cloudflared не рестартуется.** Прогоняется
   read-only `cloudflared tunnel ingress validate`; провал → **warning** в
   `SetupReport`, не ошибка (мы ничего не меняли; деплой-тайм upsert защищён
   собственным откатом).
4. **`agent.toml`.** Токен пишется как сейчас (`ensure_cloudflare_token`); секции
   `[cloudflare]`/`[cloudflared]` дописываются существующим
   `upsert_cloudflared_agent_toml` с резолвнутым `tunnel_id` (семантика «только если
   `[cloudflared]` отсутствует» сохраняется, иначе skip в отчёте).
5. **User-юнит.** Как сейчас: отсутствующий — скаффолдится, существующий — skip;
   активный юнит не перезапускается.

### 3.2. Файл есть, но не парсится / нет ключа `tunnel:`

Ошибка в `SetupReport` («config.yml существует, но не читается как cloudflared-конфиг;
поправьте или отложите файл в сторону и перезапустите setup»), **ноль записей на
диск**. `.bak`-нарушение исчезает по построению: существующий `config.yml` не
перезаписывается ни на одном пути.

### 3.3. Файла нет → свежий путь, без изменений

Текущее поведение: `find_or_create_tunnel` → creds → рендер → validate → agent.toml.

**Идемпотентность:** повторный setup на адоптированном хосте снова попадает в §3.1,
все шаги дают skip, отчёт чистый. **Dry-run** печатает план адопции и не пишет ничего.

## 4. Деплой-тайм рестарт: `XDG_RUNTIME_DIR`

В `CloudflaredIngress` (оба места спавна restart-команды — upsert и remove): если в
окружении процесса агента нет `XDG_RUNTIME_DIR`, подставить в env команды
`/run/user/<uid>`, где uid — собственный uid процесса (`libc::getuid`, за
`cfg(unix)`). Вычисление добавляемых env-переменных выносится в чистую функцию
(env процесса → доп. переменные команды) и тестируется напрямую. Уже установленная
переменная окружения и кастомная `restart`-команда из `agent.toml` не трогаются.
Ни канонический юнит, ни дефолт `default_restart()` не меняются — фикс работает на
любом uid и на уже развёрнутых хостах без переустановки юнита.

## 5. Громкость

- **Итог деплоя.** `Ingress::upsert` возвращает `IngressOutcome::{Applied, Skipped}`
  вместо `()`; `DisabledIngress` возвращает `Skipped` (текущая строка в логе
  остаётся). `DeployProject` при `Skipped` пишет `warning: …`-строку последней в деплой-лог
  (SSE-событие `finished` несёт только статус-строку, менять его формат
  несовместимо); CLI собирает строки с префиксом `warning: ` из стрима и
  повторяет их через `output::warn` рядом с итоговой сводкой. Старый CLI просто
  видит строку в конце лога, старый агент warning-строк не шлёт — совместимость
  в обе стороны. Команда включения в тексте warning'а:
  `sudo rpi agent setup --with-cloudflared --cf-token … --domain …`.
- **`rpi doctor`.** Диагностика выполняется на агенте (`/v1/doctor` →
  `RunDiagnostics` → `HostSystemProbe`), поэтому wire менять не нужно:
  `HostSystemProbe` получает флаг `ingress_active` (вычисляется при сборке
  ingress в `agent/state.rs`) и добавляет провальный чек `ingress routing`,
  когда есть зарегистрированные проекты с `hostname` при выключенном ingress —
  detail перечисляет hostname'ы, hint содержит команду включения. Generic
  `DiagnosticCheckDto` совместим со старым CLI.

## 6. Тесты

- **`setup.rs` (FakeSys):** адопция не пишет по пути config.yml (assert на
  отсутствие записи); UUID из `tunnel:` → agent.toml, ни одного вызова tunnel-API
  (мок без ожиданий); имя → adopt-ветка `find_or_create_tunnel`; `credentials-file`
  читается из конфига, не канонический; отсутствие creds-файла → ошибка, ноль
  записей; непарсящийся файл / без `tunnel:` → ошибка, ноль записей; validate-провал
  на адопции → warning, не error; повторный запуск → skip; dry-run не пишет; файла
  нет → существующие тесты свежего пути проходят без правок.
- **`cloudflared.rs`:** чистая функция env-инъекции: добавляет `XDG_RUNTIME_DIR`,
  когда его нет в env процесса; не перетирает уже установленный.
- **`deploy.rs`/`commands.rs`:** `Skipped` + hostname → `warning: …` последней
  строкой `log_tail`; `Applied` → без warning. В CLI — юнит-тест выделения
  `warning: `-префикса (`deploy_warning`). proto.rs не меняется.
- **doctor:** hostname + `disabled` → warn; `cloudflared` или нет hostname'ов →
  тишина; status без поля ingress → чек скипается.
- **Интеграционно на Pi (чеклист, руками):** setup с токеном → `config.yml`
  побайтово не изменился (сверить хеш до/после), board отвечает всё время; секции в
  `agent.toml` с верным tunnel_id; `rpi deploy` rpi-deploy-site → роут в config.yml +
  CNAME, `https://rpi.iiskelo.com` отвечает; повторный setup → skip-отчёт.

## 7. Документация (README)

- Раздел «Cloudflare Tunnel»: подраздел про адопцию существующей инсталляции — что
  setup делает (и чего не делает) с живым config.yml; требование канонического пути;
  скоупы токена: `Zone:Zone:Read + Zone:DNS:Edit` достаточно при UUID в `tunnel:`,
  `Account:Cloudflare Tunnel:Edit` — для свежих инсталляций и адопции по имени.
- Убрать/пометить устаревшими ручные обходы с `env XDG_RUNTIME_DIR=…` для рестарта.
- Упомянуть warning деплоя и doctor-чек как штатный сигнал «ingress не включён».

## 8. Раскатка на целевой Pi

Релиз 0.14.0 → обновить бинарь на Pi → `sudo rpi agent setup --with-cloudflared
--cf-token <tok> --domain iiskelo.com` (board не трогается, downtime 0) →
`rpi deploy` из rpi-deploy-site → проверить `https://rpi.iiskelo.com`.

## 9. Принятые решения

1. **Preserve-in-place, не мердж/канонизация:** существующий `config.yml` не
   перезаписывается никогда; каноничность файла самоустраняется первым же
   деплой-тайм upsert'ом.
2. **Адопция живёт в `setup`, не в `rpi agent migrate`:** миграциям нечем принять
   токен/домен; у setup флаги уже есть.
3. **validate-провал на адопции — warning:** setup ничего не менял, а рантайм
   защищён откатом; ошибка блокировала бы адопцию из-за чужих (возможно рабочих)
   экзотических конфигов.
4. **XDG-фикс — инъекция env в спавн команды**, не правка юнита/дефолта конфига:
   работает на любом uid и на уже развёрнутых хостах.
5. **Громкость — двумя каналами** (сводка деплоя + doctor), оба поля wire-совместимы
   через `#[serde(default)]`.
