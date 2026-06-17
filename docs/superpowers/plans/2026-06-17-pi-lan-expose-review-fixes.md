# pi lan-expose — План работы над замечаниями к PR #5

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Исправить замечания из ревью PR #5 (от khmilevoi): вынести блокирующий UDP-syscall из async-контекста, переписать невнятный коммит `6686588 some improvements`, показать expose-режим в `pi ls`, приподнять `UdpHostNetwork::new()` и логировать LAN-IP при старте агента.

**Architecture:** Корректируем слои (§5):
1. **Application**: в `DeployProject::run_stages` оборачиваем синхронный вызов `HostNetwork::primary_ipv4()` в `tokio::task::spawn_blocking` — трейт остаётся синхронным, мок не меняется, syscall уходит с потока экзекьютора.
2. **Application/Bin**: добавляем поле `expose` в `ProjectView` и `ProjectViewDto`, колонку `EXPOSE` в рендер `pi ls`.
3. **Infrastructure/Bin**: `UdpHostNetwork::new()` возвращает bare ZST, коллеры сами оборачивают в `Arc`; проброс `host_network` в `AppState` для startup-лога в `run.rs`.

**Tech Stack:** Rust, Tokio, axum, serde, mockall (automock).

---

## Сводка замечаний

| № | Приоритет | Файл / Место | Суть проблемы | Источник |
|---|---|---|---|---|
| 1 | 🟡 До мержа | `hostnet.rs:14-22` → `deploy.rs:276` | Блокирующий UDP-syscall (`UdpSocket::bind`/`connect`) в async-контексте `run_stages` | khmilevoi |
| 2 | 🟡 До мержа | коммит `6686588` | Невнятное сообщение коммита `some improvements` | khmilevoi |
| 3 | 🔵 На усмотрение | `list.rs:9-16`, `proto.rs:330-357`, `cli/commands.rs:163-184` | `pi ls` не показывает expose-режим проекта | khmilevoi |
| 4 | 🔵 На усмотрение | `hostnet.rs:9-11` | `UdpHostNetwork::new()` оборачивает ZST в `Arc` | khmilevoi |
| 5 | 🔵 Рекомендация | `run.rs` / `state.rs` | LAN-IP не логируется при старте агента (только на deploy) | khmilevoi |

---

## План исправления

### Task 1: Вынести UDP-syscall в `spawn_blocking` (🟡 До мержа)

`HostNetwork::primary_ipv4` — синхронный трейт-метод (`crates/domain/src/contracts.rs:89-91`), вызывается в async `run_stages` (`crates/application/src/deploy.rs:276`). Обернём вызов в `tokio::task::spawn_blocking` на сайте вызова: трейт и `MockHostNetwork` (automock) не меняются, syscall уходит в blocking-pool. `Arc<dyn HostNetwork>` — `Send + Sync + 'static`, замыкание корректно.

**Files:**
- Modify: `crates/application/src/deploy.rs:275-281`

- [ ] **Step 1: Зафиксировать базовое поведение существующими тестами**

Поведение не должно измениться — тесты `lan_deploy_logs_reachable_url` и `lan_deploy_logs_port_when_ip_not_detected` (`deploy.rs:608-686`) уже покрывают оба пути. Прогнать перед изменением:
```bash
rtk cargo test -p pi-application deploy::tests::lan_deploy_logs
```
Expected: 2 passed.

- [ ] **Step 2: Обернуть вызов в `spawn_blocking`**

В `crates/application/src/deploy.rs:275-281` заменить:
```rust
        if config.expose == ExposeMode::Lan {
            if let Some(ip) = self.host_network.primary_ipv4() {
                log.line(&format!("lan: http://{ip}:{}", project.host_port));
            } else {
                log.line(&format!("lan: {} (ip not detected)", project.host_port));
            }
        }
```
на:
```rust
        if config.expose == ExposeMode::Lan {
            let hn = Arc::clone(&self.host_network);
            let ip = tokio::task::spawn_blocking(move || hn.primary_ipv4())
                .await
                .ok()
                .flatten();
            if let Some(ip) = ip {
                log.line(&format!("lan: http://{ip}:{}", project.host_port));
            } else {
                log.line(&format!("lan: {} (ip not detected)", project.host_port));
            }
        }
```

- [ ] **Step 3: Прогнать тесты и закоммитить**

```bash
rtk cargo test -p pi-application deploy
rtk git add -A && rtk git commit -m "fix(deploy): offload UDP primary_ipv4 syscall to spawn_blocking"
```
Expected: все deploy-тесты зелёные (поведение сохранено, mock отрабатывает на blocking-потоке).

---

### Task 2: Переписать невнятное сообщение коммита `6686588` (🟡 До мержа)

Коммит `6686588 some improvements` добавил проброс `HostNetwork` в `DeployProject` (поле, параметр конструктора, мок) и логирование reachable LAN-URL в `run_stages` + 3 LAN-теста. Сообщение не отражает суть.

**Важно:** `6686588` — не верхушка ветки, поверх неё есть merge-коммиты (`ad03c9f`, `f10a3b1`). Rebase для reword рискован. Поэтому два пути — выберите один на execution-чекпоинте.

- [ ] **Step 1 (вариант A — рекомендуемый): squash-merge при мерже PR #5**

При мерже PR использовать **Squash merge** с итоговым сообщением, описывающим всю ветку:
```
feat(lan-expose): expose=lan mode, reachable URL logging, host network wiring
```
Это схлопнет `6686588` и merge-коммиты в один чистый. История `master` останется линейной. Никаких действий в worktree не требуется.

- [ ] **Step 1 (вариант B): reword через интерактивный rebase**

Если squash-merge неприемлем:
```bash
rtk git rebase -i <base-ветка>
```
В открывшемся списке пометить `6686588` как `reword`, задать сообщение:
```
feat(deploy): wire HostNetwork into deploy, log reachable LAN url for expose=lan
```
Затем force-push:
```bash
rtk git push --force-with-lease origin codex-pi-lan-expose
```
**Риск:** переписываются merge-коммиты `ad03c9f`/`f10a3b1` — убедитесь, что никто другой не ведёт работу поверх этой ветки.

- [ ] **Step 2: Проверить историю**

```bash
rtk git log --oneline -8
```
Expected: сообщение `6686588` переписано (вариант B) либо ветка готова к squash-merge (вариант A).

---

### Task 3: Показывать expose-режим в `pi ls` (🔵 На усмотрение)

`ProjectView` (`crates/application/src/list.rs:9-16`) и `ProjectViewDto` (`crates/bin/src/proto.rs:330-337`) не содержат `expose`, поэтому пользователи не видят, какой проект приватный, а какой — LAN.

**Files:**
- Modify: `crates/application/src/list.rs:9-16, 41-48, 63-80`
- Modify: `crates/bin/src/proto.rs:330-357`
- Modify: `crates/bin/src/cli/commands.rs:163-184`
- Test: `crates/application/src/list.rs` (тесты), `crates/bin/src/proto.rs` (roundtrip)

- [ ] **Step 1: Написать падающий тест на `expose` в `ProjectView`**

В `crates/application/src/list.rs` в тесте `lists_projects_with_service_states` (:82) добавить проверку поля:
```rust
        assert_eq!(views[0].expose, ExposeMode::default());
```
и в хелпере `project()` (:63-80) поле уже есть `expose: ExposeMode::default()` — добавить его в конструктор `ProjectView` в `execute()` (см. Step 3). Сначала тест упадёт из-за отсутствия поля.

- [ ] **Step 2: Прогнать тест, убедиться в ошибке компиляции**

```bash
rtk cargo test -p pi-application list
```
Expected: FAIL / compile error — `no field expose on type ProjectView`.

- [ ] **Step 3: Добавить поле `expose` в `ProjectView` и заполнять его**

В `crates/application/src/list.rs:9-16` добавить поле:
```rust
pub struct ProjectView {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub hostname: Option<String>,
    pub host_port: u16,
    pub expose: ExposeMode,
    pub services: Vec<ServiceState>,
}
```
Импорт: `use pi_domain::entities::{ExposeMode, ServiceState};` (добавить `ExposeMode` к существующему импорту `ServiceState` на :4).

В `execute()` (:41-48) добавить:
```rust
            views.push(ProjectView {
                name: project.config.name,
                repo: project.config.repo,
                branch: project.config.branch,
                hostname: project.config.hostname,
                host_port: project.host_port,
                expose: project.config.expose,
                services,
            });
```

- [ ] **Step 4: Прогнать application-тесты**

```bash
rtk cargo test -p pi-application list
```
Expected: PASS.

- [ ] **Step 5: Добавить `expose` в `ProjectViewDto` и конверсию**

В `crates/bin/src/proto.rs:330-337`:
```rust
pub struct ProjectViewDto {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub hostname: Option<String>,
    pub host_port: u16,
    pub expose: String,
    pub services: Vec<ServiceStateDto>,
}
```
В `From<ProjectView> for ProjectViewDto` (:339-357) добавить:
```rust
            expose: v.expose.as_str().to_string(),
```
(`ExposeMode::as_str` уже используется в `proto.rs:93` и определён в `crates/domain/src/entities.rs:98`.)

- [ ] **Step 6: Добавить roundtrip-тест в proto.rs**

В `crates/bin/src/proto.rs` в `mod tests` добавить:
```rust
    #[test]
    fn project_view_dto_exposes_expose_mode_string() {
        let view = ProjectView {
            name: "a".into(),
            repo: "r".into(),
            branch: "main".into(),
            hostname: None,
            host_port: 8000,
            expose: pi_domain::entities::ExposeMode::Lan,
            services: vec![],
        };
        let dto = ProjectViewDto::from(view);
        assert_eq!(dto.expose, "lan");
    }
```

- [ ] **Step 7: Добавить колонку `EXPOSE` в рендер `pi ls`**

В `crates/bin/src/cli/commands.rs:163-184` заменить заголовок и строку:
```rust
    println!(
        "{:<16} {:<10} {:<8} {:<28} {:<6} SERVICES",
        "NAME", "BRANCH", "EXPOSE", "HOSTNAME", "PORT"
    );
    for p in projects {
        let services = if p.services.is_empty() {
            "-".to_string()
        } else {
            p.services
                .iter()
                .map(|s| format!("{}:{}", s.service, s.state))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "{:<16} {:<10} {:<8} {:<28} {:<6} {services}",
            p.name,
            p.branch,
            p.expose,
            p.hostname.unwrap_or_else(|| "-".into()),
            p.host_port
        );
    }
```

- [ ] **Step 8: Собрать и прогнать весь bin-крейт**

```bash
rtk cargo test -p pi-bin
```
Expected: PASS (включая новый proto-тест).

- [ ] **Step 9: Визуально проверить рендер (ручная проверка)**

```bash
rtk cargo run -p pi-bin -- ls
```
Expected: в таблице присутствует колонка `EXPOSE` со значениями `private`/`lan`.

- [ ] **Step 10: Закоммитить**

```bash
rtk git add -A && rtk git commit -m "feat(ls): show expose mode column in pi ls"
```

---

### Task 4: Приподнять `UdpHostNetwork::new()` — bare ZST (🔵 На усмотрение)

`UdpHostNetwork` — zero-sized тип (без полей), но `new()` возвращает `Arc<UdpHostNetwork>`. Сделаем `new()` честным конструктором ZST, коллеры сами оборачивают в `Arc<dyn HostNetwork>` (unsized coercion работает на сайте вызова `DeployProject::new`).

**Files:**
- Modify: `crates/infrastructure/src/hostnet.rs:8-12`
- Modify: `crates/bin/src/agent/state.rs:91`
- Modify: `crates/bin/src/agent/http.rs:587`

- [ ] **Step 1: Изменить `new()` на bare тип**

В `crates/infrastructure/src/hostnet.rs:8-12`:
```rust
impl UdpHostNetwork {
    pub fn new() -> UdpHostNetwork {
        UdpHostNetwork
    }
}
```
Тест (:31-33) `let network = UdpHostNetwork::new(); network.primary_ipv4();` остаётся корректным (метод вызывается на bare типе через `impl HostNetwork for UdpHostNetwork`).

- [ ] **Step 2: Обновить коллеров**

`crates/bin/src/agent/state.rs:91`:
```rust
        Arc::new(UdpHostNetwork::new()),
```
`crates/bin/src/agent/http.rs:587`:
```rust
            Arc::new(UdpHostNetwork::new()),
```
`Arc<UdpHostNetwork>` coerces to `Arc<dyn HostNetwork>` в позиции аргумента `DeployProject::new`.

- [ ] **Step 3: Собрать и прогнать**

```bash
rtk cargo build -p pi-bin
rtk cargo test -p pi-infrastructure hostnet
```
Expected: PASS.

- [ ] **Step 4: Закоммитить**

```bash
rtk git add -A && rtk git commit -m "refactor(hostnet): return bare ZST from UdpHostNetwork::new"
```

---

### Task 5: Логировать LAN-IP при старте агента (🔵 Рекомендация)

Сейчас LAN-IP определяется только на `pi deploy` (`deploy.rs:276`). Если на хосте нет non-loopback IPv4, пользователь узнаёт об этом только после полного деплоя. Добавим одноразовый лог в `run.rs` при старте, пробросив `host_network` через `AppState`.

**Files:**
- Modify: `crates/bin/src/agent/state.rs:36-54, 81-96, 146-163`
- Modify: `crates/bin/src/agent/run.rs:48-57`

- [ ] **Step 1: Добавить `host_network` в `AppState`**

В `crates/bin/src/agent/state.rs:36-54` добавить поле в структуру:
```rust
    pub host_network: Arc<dyn pi_domain::contracts::HostNetwork>,
```
Импорт `HostNetwork` добавить к `use pi_domain::contracts::{DeploymentHistory, IdGen, Ingress};` (:16) — заменить на:
```rust
use pi_domain::contracts::{DeploymentHistory, HostNetwork, IdGen, Ingress};
```

- [ ] **Step 2: Создать `Arc<dyn HostNetwork>` и переиспользовать**

В `build_state` (:81-96) перед `DeployProject::new`:
```rust
    let host_network: Arc<dyn HostNetwork> = UdpHostNetwork::new();
```
(После Task 4 `UdpHostNetwork::new()` возвращает bare тип — здесь нужно `Arc::new(UdpHostNetwork::new())`. Если Task 4 не выполнен — `UdpHostNetwork::new()` уже даёт `Arc`.)
Передать в `DeployProject::new` `Arc::clone(&host_network)` вместо `UdpHostNetwork::new()` (:91), и добавить в `AppState` `host_network: Arc::clone(&host_network)` (:146-163).

- [ ] **Step 3: Логировать IP в `run.rs`**

В `crates/bin/src/agent/run.rs:48` после `let state = build_state(&config, log_dir_available)?;` добавить:
```rust
    match state.host_network.primary_ipv4() {
        Some(ip) => tracing::info!("lan ip detected: {ip}"),
        None => tracing::warn!(
            "no non-loopback ipv4 detected; lan-exposed projects will log port only"
        ),
    }
```

- [ ] **Step 4: Собрать и прогнать**

```bash
rtk cargo build -p pi-bin
rtk cargo test
```
Expected: PASS.

- [ ] **Step 5: Закоммитить**

```bash
rtk git add -A && rtk git commit -m "feat(agent): log detected lan ipv4 at startup"
```

---

## Финальная верификация

- [ ] **Полный набор тестов + линтеры**

```bash
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
rtk cargo fmt --check
```
Expected: все тесты зелёные, clippy чист, fmt без изменений.

- [ ] **Проверить, что 78/78 тестов из ревью по-прежнему проходят**

В ревью зафиксировано 78/78. После правок ожидается 78 +新增 (proto expose test) ≥ 79.

---

## Порядок выполнения

Task 1 → Task 3 → Task 4 → Task 5 → финальная верификация. Task 2 (git-история) выполняется отдельно в конце — либо squash-merge при мерже PR (вариант A), либо rebase перед мержем (вариант B).

Зависимости: Task 5 ссылается на результат Task 4 (`UdpHostNetwork::new()` возвращает bare тип) — если Task 4 пропущен, Step 2 Task 5 использует `UdpHostNetwork::new()` напрямую (уже `Arc`).
