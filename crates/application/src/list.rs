use std::sync::Arc;

use pi_domain::contracts::{ContainerRuntime, ProjectRepository};
use pi_domain::entities::ServiceState;
use pi_domain::error::DomainError;

/// Строка для `pi ls`: проект + состояние его сервисов (§7 ListProjects).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectView {
    pub name: String,
    pub repo: String,
    pub branch: String,
    pub hostname: Option<String>,
    pub host_port: u16,
    pub services: Vec<ServiceState>,
}

pub struct ListProjects {
    projects: Arc<dyn ProjectRepository>,
    runtime: Arc<dyn ContainerRuntime>,
}

impl ListProjects {
    pub fn new(
        projects: Arc<dyn ProjectRepository>,
        runtime: Arc<dyn ContainerRuntime>,
    ) -> Arc<ListProjects> {
        Arc::new(ListProjects { projects, runtime })
    }

    pub async fn execute(&self) -> Result<Vec<ProjectView>, DomainError> {
        let mut views = Vec::new();
        for project in self.projects.list().await? {
            // Ошибка ps (стек ещё не поднимался / docker недоступен) - не повод
            // ронять весь список: показываем проект без сервисов.
            let services = self.runtime.ps(&project.config.name).await.unwrap_or_default();
            views.push(ProjectView {
                name: project.config.name,
                repo: project.config.repo,
                branch: project.config.branch,
                hostname: project.config.hostname,
                host_port: project.host_port,
                services,
            });
        }
        Ok(views)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_domain::contracts::{MockContainerRuntime, MockProjectRepository};
    use pi_domain::entities::{Project, ProjectConfig, ServiceState};
    use pi_domain::error::DomainError;

    fn project(name: &str, host_port: u16) -> Project {
        Project {
            config: ProjectConfig {
                name: name.into(),
                repo: format!("git@github.com:x/{name}.git"),
                branch: "main".into(),
                compose_path: "docker-compose.yml".into(),
                service: "web".into(),
                container_port: 3000,
                hostname: None,
            },
            host_port,
            created_at: 1,
        }
    }

    #[tokio::test]
    async fn lists_projects_with_service_states() {
        let mut projects = MockProjectRepository::new();
        projects
            .expect_list()
            .returning(|| Ok(vec![project("a", 8000), project("b", 8001)]));

        let mut runtime = MockContainerRuntime::new();
        runtime.expect_ps().withf(|n| n == "a").returning(|_| {
            Ok(vec![ServiceState {
                service: "web".into(),
                state: "running".into(),
            }])
        });
        // ps по "b" падает (стек ни разу не поднимался) - статус unknown, не ошибка
        runtime.expect_ps().withf(|n| n == "b").returning(|_| {
            Err(DomainError::Runtime("no such project".into()))
        });

        let list = ListProjects::new(Arc::new(projects), Arc::new(runtime));
        let views = list.execute().await.unwrap();

        assert_eq!(views.len(), 2);
        assert_eq!(views[0].name, "a");
        assert_eq!(views[0].host_port, 8000);
        assert_eq!(
            views[0].services,
            vec![ServiceState {
                service: "web".into(),
                state: "running".into()
            }]
        );
        assert_eq!(views[1].name, "b");
        assert!(views[1].services.is_empty());
    }
}
