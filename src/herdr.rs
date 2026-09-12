use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

pub fn repository_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return crate::git::discover(path);
    }
    if let Some(path) = env::var_os("HERDR_GIT_GRAPH_REPO") {
        return crate::git::discover(Path::new(&path));
    }
    let cwd = env::current_dir()?;
    let plugin_root = env::var_os("HERDR_PLUGIN_ROOT").map(PathBuf::from);
    // A pane opened with --cwd takes precedence over the source pane's context.
    if plugin_root.as_ref() != Some(&cwd)
        && let Ok(root) = crate::git::discover(&cwd)
    {
        return Ok(root);
    }
    let context = plugin_context();
    for path in context_paths(&context) {
        if let Ok(root) = crate::git::discover(&path) {
            return Ok(root);
        }
    }
    crate::git::discover(&cwd)
}

fn plugin_context() -> Value {
    env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or(Value::Null)
}

fn context_id(context: &Value, key: &str, fallback_env: &str) -> Option<String> {
    context
        .get(key)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(|| env::var(fallback_env).ok().filter(|id| !id.is_empty()))
}

pub fn context_paths(context: &Value) -> Vec<PathBuf> {
    [
        context.get("focused_pane_cwd"),
        context.pointer("/worktree/checkout_path"),
        context.get("workspace_cwd"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .filter(|s| !s.is_empty())
    .map(PathBuf::from)
    .collect()
}

pub fn open_pane(root: &Path) -> Result<()> {
    open_view(root, false)
}

pub fn open_sidebar(root: &Path) -> Result<()> {
    open_view(root, true)
}

fn open_view(root: &Path, sidebar: bool) -> Result<()> {
    let context = plugin_context();
    let bin = env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let mut command = Command::new(bin);
    command
        .args([
            "plugin",
            "pane",
            "open",
            "--plugin",
            "herdr.git-graph",
            "--entrypoint",
            if sidebar { "sidebar" } else { "graph" },
            "--placement",
            if sidebar { "split" } else { "tab" },
            if sidebar { "--no-focus" } else { "--focus" },
            "--cwd",
        ])
        .arg(root)
        .arg("--env")
        .arg(format!("HERDR_GIT_GRAPH_REPO={}", root.display()));
    if sidebar {
        // Herdr actions carry their source pane in the invocation context.
        // Split placement accepts a target pane, but rejects workspace_id.
        let id = context_id(&context, "focused_pane_id", "HERDR_PANE_ID")
            .context("사이드바를 열 대상 패널이 없습니다. Herdr 패널 안에서 액션을 실행하세요.")?;
        command
            .args(["--direction", "right", "--target-pane"])
            .arg(id);
    } else if let Some(id) = context_id(&context, "workspace_id", "HERDR_WORKSPACE_ID") {
        command.arg("--workspace").arg(id);
    }
    let status = command.status().context("Herdr를 실행할 수 없습니다.")?;
    if !status.success() {
        bail!("Herdr Git Graph 패널을 열지 못했습니다. 플러그인 등록을 확인하세요.");
    }
    Ok(())
}

#[cfg(unix)]
pub mod socket {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };

    #[derive(Clone)]
    pub struct Endpoint {
        pub path: PathBuf,
        pub pane: String,
    }

    impl Endpoint {
        pub fn from_env() -> Result<Self> {
            Ok(Self {
                path: env::var_os("HERDR_SOCKET_PATH")
                    .context("Herdr 소켓이 없습니다.")?
                    .into(),
                pane: env::var("HERDR_PANE_ID").context("Herdr 패널 ID가 없습니다.")?,
            })
        }

        pub fn connect(&self) -> Result<UnixStream> {
            let stream = UnixStream::connect(&self.path).context("Herdr 소켓 연결 실패")?;
            stream.set_read_timeout(Some(Duration::from_millis(700)))?;
            stream.set_write_timeout(Some(Duration::from_millis(700)))?;
            Ok(stream)
        }

        pub fn request_on(
            &self,
            stream: &mut UnixStream,
            method: &str,
            params: Value,
        ) -> Result<Value> {
            serde_json::to_writer(
                &mut *stream,
                &json!({"id":"git-graph", "method":method, "params":params}),
            )?;
            stream.write_all(b"\n")?;
            let mut response = String::new();
            BufReader::new(stream.take(1024 * 1024)).read_line(&mut response)?;
            let response: Value = serde_json::from_str(&response).context("잘못된 Herdr 응답")?;
            if let Some(error) = response.get("error") {
                bail!("Herdr: {error}");
            }
            response
                .get("result")
                .cloned()
                .context("Herdr 응답에 result가 없습니다.")
        }

        pub fn request(&self, method: &str, params: Value) -> Result<Value> {
            self.request_on(&mut self.connect()?, method, params)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worktree_context_uses_checkout_not_shared_repo_root() {
        let paths = context_paths(
            &json!({"focused_pane_cwd":"/repo/sub dir", "worktree":{"checkout_path":"/worktrees/feature", "repo_root":"/repo"}, "workspace_cwd":"/workspace"}),
        );
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/repo/sub dir"),
                PathBuf::from("/worktrees/feature"),
                PathBuf::from("/workspace")
            ]
        );
    }
}
