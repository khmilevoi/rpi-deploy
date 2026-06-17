# pi — LAN-expose (`expose = "lan"`) (дизайн)

Дата: 2026-06-17. Базовая спека: `2026-06-09-pi-deploy-tool-design.md` (§12 pi.toml,
§12.1 compose-override, §11 ingress).

**Критерий готовности:** проект с `[ingress] expose = "lan"` доступен с других
машин локальной сети по `http://<lan-ip-пайки>:<host-port>`; проект без этой
настройки ведёт себя ровно как раньше (бинд только на `127.0.0.1`).

## 1. Скоуп

Входит:

- Новый per-project переключатель `[ingress] expose = "private" | "lan"` в
  `pi.toml`; дефолт `private` (текущее поведение).
- При `lan` агент биндит host-порт на `0.0.0.0` вместо `127.0.0.1` в
  compose-override (§12.1).
- Агент определяет свой основной LAN-IPv4 и отдаёт его клиенту; `pi deploy` и
  `pi ls` показывают готовый `http://<ip>:<host-port>` и пометку `expose=lan`.
- Персист `expose` в реестре проектов (миграция БД), чтобы `pi ls` показывал
  режим без повторного деплоя.

НЕ входит (YAGNI):

- Управление фаерволом со стороны pi. За изоляцию от интернета отвечает роутер
  (NAT/отсутствие проброса портов). Решение пользователя.
- Бинд на конкретный IP/интерфейс или подсеть. Только `127.0.0.1` ↔ `0.0.0.0`.
- Явный `lan_ip` в `agent.toml` как ручной фолбэк определения IP — возможное
  будущее расширение, не в этой версии.
- Отдача нескольких портов. Как и сейчас, мапится единственный ingress-порт
  проекта.

## 2. Ключевые решения

1. **`0.0.0.0`, фаервол не трогаем.** Простейшая модель: бинд на все интерфейсы.
   В домашней сети без проброса портов это де-факто только LAN. pi не управляет
   ufw/nftables.
2. **`lan` ортогонален Cloudflare.** `0.0.0.0` включает loopback, поэтому туннель
   (ходит на `127.0.0.1:<host-port>`, §11) продолжает работать. Проект может быть
   одновременно LAN и публичным; `hostname` и `expose` независимы.
3. **LAN-IP определяет агент, не CLI.** CLI работает на dev-машине и не знает IP
   пайки. Агент определяет основной IPv4 трюком с UDP-сокетом: `connect` на
   `8.8.8.8:80` (пакеты не отправляются), чтение `local_addr()` → IP интерфейса
   исходящего маршрута. Кроссплатформенно, без новых зависимостей, выбирает
   основной интерфейс.
4. **Деградация без падения.** Если IP определить не удалось (`None`) — деплой и
   `pi ls` не падают: показываем порт + пометку `expose=lan` и строку вида
   `lan: <host-port> (ip not detected)`.
5. **Дефолт = `private`.** Имя выбрано для однозначного контраста с `lan`
   (`private` = доступно только на хосте; `lan` = доступно в сети). Старые
   `pi.toml` и старые wire-payload'ы без `expose` трактуются как `private`.
6. **Healthcheck без изменений.** Гейт по-прежнему стучит на `127.0.0.1:<host-port>`
   (§8); `0.0.0.0` включает loopback, так что менять нечего.

## 3. Поверхность: `pi.toml` и вывод

`pi.toml`, секция `[ingress]` — новое опциональное поле:

```toml
[ingress]
hostname = "app.example.com"   # опц., как раньше
service  = "web"
port     = 3000
expose   = "lan"               # "private" (дефолт) | "lan"
```

`pi ls` (человекочитаемый вывод) для lan-проекта добавляет к строке проекта
пометку `expose=lan` и URL:

```
example-web  main  8000  expose=lan  http://192.168.1.50:8000  [web: running]
```

Для `private`-проекта вывод не меняется (пометки нет).

`pi deploy` после health-гейта печатает в лог одну строку:

```
lan: http://192.168.1.50:8000
```

или, если IP не определился:

```
lan: 8000 (ip not detected)
```

## 4. Domain

`entities.rs`:

- `enum ExposeMode { Private, Lan }`, `Default = Private`.
- `ExposeMode::bind_addr(&self) -> &'static str` → `"127.0.0.1"` / `"0.0.0.0"`.
- `ExposeMode::as_str(&self) -> &'static str` → `"private"` / `"lan"` (для БД и
  wire); `FromStr` с валидацией.
- Поле `pub expose: ExposeMode` в `ProjectConfig`.

`contracts.rs`:

- Новый контракт `HostNetwork: Send + Sync { fn primary_ipv4(&self) -> Option<IpAddr> }`
  (порт определения LAN-IP). Под `#[cfg_attr(feature = "mocks", automock)]`.
- `OverrideStore::write` принимает bind-адрес `&str` (новый параметр); обновить
  doc-комментарий (сейчас он фиксирует `127.0.0.1`).

## 5. Infrastructure

- `overrides.rs`: сигнатура `override_yaml(service, bind, host_port, container_port)`,
  строка мэппинга — `"{bind}:{host_port}:{container_port}"`. `FsOverrideStore::write`
  прокидывает bind.
- Новый `UdpHostNetwork` — реализация `HostNetwork` через UDP-connect трюк;
  `primary_ipv4` возвращает `None` при любой ошибке сокета.
- `sqlite.rs`: миграция №2 —
  `ALTER TABLE projects ADD COLUMN expose TEXT NOT NULL DEFAULT 'private';`
  Существующие строки получают `private`.
- `repo.rs`: `SELECT` (+ `INSERT`/`UPDATE`) включают `expose`; маппинг строки →
  `Project` парсит `expose` (неизвестное значение → ошибка `Storage`, но на
  практике пишем только валидные).

## 6. Application

- `deploy.rs` (`run_stages`): передавать `config.expose.bind_addr()` в
  `overrides.write`. После health-гейта, если `config.expose == Lan`, дёрнуть
  `host_network.primary_ipv4()` и записать lan-строку (решение 4). Добавить
  зависимость `host_network: Arc<dyn HostNetwork>` в `Deploy`.
- `list.rs`: `ProjectView` получает `expose: ExposeMode` и `lan_ip: Option<IpAddr>`.
  `ListProjects` получает `host_network: Arc<dyn HostNetwork>`, дёргает
  `primary_ipv4()` один раз на вызов `execute()` и проставляет `lan_ip` только
  lan-проектам.

## 7. Wire + CLI

`proto.rs`:

- `ProjectDto`: `#[serde(default)] expose: Option<String>` (None/отсутствие →
  `private` при конверсии в `ProjectConfig`). `From<&ProjectConfig>` пишет
  `Some(expose.as_str())`.
- `ProjectViewDto`: поля `#[serde(default)] expose: Option<String>` и
  `#[serde(default)] lan_ip: Option<String>`.
- `From<ProjectView>` заполняет оба.

`http.rs`: маппинг `ProjectView → ProjectViewDto` уже идёт через `From`; правок
логики нет, только проброс новых полей.

`cli/pitoml.rs`:

- `IngressSection`: `#[serde(default)] expose: Option<String>`.
- В `PiToml::parse` валидировать значение (`"private"`/`"lan"`), иначе
  `anyhow::bail!` с подсказкой.
- `to_project_config` мапит в `ExposeMode` (None → `Private`).

`cli/commands.rs`: рендер `pi ls` — для lan-проекта добавить `expose=lan` и, если
`lan_ip` есть, `http://<ip>:<port>`.

## 8. Тесты

- `override_yaml`: отдельные кейсы для `private` (`127.0.0.1`) и `lan` (`0.0.0.0`).
- `ExposeMode`: `FromStr`/`as_str`/`bind_addr`/дефолт; невалидное значение → ошибка.
- `pitoml`: парсинг `expose`, дефолт при отсутствии, отказ на мусоре.
- Миграция: открыть БД со старой схемой/строкой → после миграции `expose='private'`.
- `repo`: roundtrip проекта с `expose='lan'`; апдейт сохраняет/меняет `expose`.
- `deploy`: при `Lan` `overrides.write` зовётся с `0.0.0.0` и пишется lan-строка;
  при `None` от `HostNetwork` — строка-фолбэк, деплой успешен.
- `list`: lan-проект получает `lan_ip`, private — `None`; ошибка/`None`
  `HostNetwork` не роняет список.
- `proto`: roundtrip `expose` через DTO; payload без `expose` → `private`
  (обратная совместимость, как у healthcheck/timeouts).

## 9. Документация (README)

- Описать `[ingress] expose` и значения, добавить пример lan-проекта.
- **Предупреждение по безопасности:** `expose = "lan"` открывает порт на всех
  интерфейсах; на пайке с публичным IP без NAT это означает публичный доступ.
- **Заметка про Docker + iptables:** публикация на `0.0.0.0` на Linux добавляет
  правила iptables, которые могут обходить host-фаервол вроде ufw — это ожидаемо
  при выбранной модели (pi фаервол не трогает).
- Дополнить раздел про compose-override (показать, что бинд зависит от `expose`).
