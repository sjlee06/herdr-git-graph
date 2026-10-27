//! Wrap the existing pane tree without `layout.apply`, which recreates PTYs.
//! Socket calls avoid spawning a CLI process for every temporary pane move.
use super::sidebar::request;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{env, ffi::OsStr, path::PathBuf, time::Duration};

#[cfg(unix)]
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
};

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Pane(String),
    Split {
        direction: &'static str,
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    fn parse(value: &Value) -> Result<Self> {
        match value["type"].as_str() {
            Some("pane") => Ok(Self::Pane(string(value, "pane_id")?.into())),
            Some("split") => {
                let direction = match value["direction"].as_str() {
                    Some("right") => "right",
                    Some("down") => "down",
                    _ => bail!("알 수 없는 Herdr 분할 방향입니다."),
                };
                let ratio = value["ratio"].as_f64().context("분할 비율이 없습니다.")?;
                ensure!(
                    (0.1..=0.9).contains(&ratio),
                    "잘못된 Herdr 분할 비율입니다."
                );
                Ok(Self::Split {
                    direction,
                    ratio,
                    first: Box::new(Self::parse(&value["first"])?),
                    second: Box::new(Self::parse(&value["second"])?),
                })
            }
            _ => bail!("Herdr 분할 트리를 읽지 못했습니다."),
        }
    }

    fn first_pane(&self) -> &str {
        match self {
            Self::Pane(id) => id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    fn panes<'a>(&'a self, panes: &mut Vec<&'a str>) {
        match self {
            Self::Pane(id) => panes.push(id),
            Self::Split { first, second, .. } => {
                first.panes(panes);
                second.panes(panes);
            }
        }
    }

    fn without(self, pane: &str) -> Option<Self> {
        match self {
            Self::Pane(id) => (id != pane).then_some(Self::Pane(id)),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => match (first.without(pane), second.without(pane)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    direction,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (first, second) => first.or(second),
            },
        }
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("Herdr 응답에 {key} 값이 없습니다."))
}

/// The flat CLI layout is enough to recognize an already-correct sidebar.
fn is_outer_sidebar(layout: &Value, sidebar: &str) -> bool {
    let Some(panes) = layout["panes"].as_array() else {
        return false;
    };
    let Some(rect) = panes
        .iter()
        .find(|p| p["pane_id"] == sidebar)
        .map(|p| &p["rect"])
    else {
        return false;
    };
    let Some(x) = rect["x"].as_u64() else {
        return false;
    };
    let Some(y) = rect["y"].as_u64() else {
        return false;
    };
    let Some(width) = rect["width"].as_u64() else {
        return false;
    };
    let Some(height) = rect["height"].as_u64() else {
        return false;
    };
    width > 0 && height > 0 && panes.iter().filter(|p| p["pane_id"] != sidebar).all(|p| {
        let r = &p["rect"];
        matches!((r["x"].as_u64(), r["y"].as_u64(), r["width"].as_u64(), r["height"].as_u64()),
            (Some(px), Some(py), Some(pw), Some(ph)) if px + pw <= x && py >= y && py + ph <= y + height)
    })
}

#[cfg(unix)]
struct Socket {
    path: PathBuf,
    serial: u64,
}

#[cfg(unix)]
impl Socket {
    fn from_env() -> Result<Self> {
        let path = env::var_os("HERDR_SOCKET_PATH").context("Herdr 소켓 경로가 없습니다.")?;
        Ok(Self {
            path: path.into(),
            serial: 0,
        })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.serial += 1;
        let id = format!("git-graph-layout-{}", self.serial);
        let mut message =
            serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))?;
        message.push(b'\n');
        // Herdr closes ordinary RPC connections after one response.
        let mut stream = UnixStream::connect(&self.path)
            .context("Herdr 레이아웃 소켓에 연결하지 못했습니다.")?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        stream.write_all(&message)?;
        let mut line = String::new();
        ensure!(
            BufReader::new(stream).read_line(&mut line)? > 0,
            "Herdr 레이아웃 연결이 종료되었습니다."
        );
        let response: Value = serde_json::from_str(&line)?;
        ensure!(
            response["id"] == id,
            "Herdr 레이아웃 응답 ID가 일치하지 않습니다."
        );
        if let Some(error) = response.get("error") {
            bail!(
                "{method}: {}",
                error["message"].as_str().unwrap_or("Herdr 요청 실패")
            );
        }
        response
            .get("result")
            .cloned()
            .context("Herdr 레이아웃 응답이 없습니다.")
    }
}

pub(super) struct Layout {
    #[cfg(unix)]
    socket: Socket,
    tab: String,
    workspace: String,
    focused: String,
    root: Node,
}

impl Layout {
    /// Single-pane opens keep the existing CLI-only fast path and compatibility.
    pub(super) fn capture(
        bin: &OsStr,
        source: &str,
        sidebar: Option<&str>,
    ) -> Result<Option<Self>> {
        let response = request(bin, &["pane", "layout", "--pane", source])?;
        let snapshot = &response["result"]["layout"];
        let panes = snapshot["panes"]
            .as_array()
            .context("Herdr 패널 목록이 없습니다.")?;
        if sidebar.is_some_and(|id| is_outer_sidebar(snapshot, id))
            || (sidebar.is_none() && panes.len() == 1)
        {
            return Ok(None);
        }
        ensure!(
            snapshot["zoomed"] != true,
            "사이드바를 배치하려면 패널 확대를 해제하세요."
        );
        #[cfg(unix)]
        {
            let mut socket = Socket::from_env()?;
            let exported = socket.call("layout.export", json!({"pane_id": source}))?;
            let layout = &exported["layout"];
            ensure!(
                layout["zoomed"] != true,
                "사이드바를 배치하려면 패널 확대를 해제하세요."
            );
            let mut root = Node::parse(&layout["root"])?;
            if let Some(sidebar) = sidebar {
                root = root
                    .without(sidebar)
                    .context("사이드바 외의 작업 패널이 없습니다.")?;
            }
            let mut ids = Vec::new();
            root.panes(&mut ids);
            let count = ids.len();
            ids.sort_unstable();
            ids.dedup();
            ensure!(
                ids.len() == count,
                "Herdr 레이아웃에 중복된 패널이 있습니다."
            );
            Ok(Some(Self {
                socket,
                tab: string(layout, "tab_id")?.into(),
                workspace: string(layout, "workspace_id")?.into(),
                focused: string(layout, "focused_pane_id")?.into(),
                root,
            }))
        }
        #[cfg(not(unix))]
        bail!("전체 높이 사이드바는 Unix Herdr 소켓이 필요합니다.")
    }

    pub(super) fn anchor(&self) -> &str {
        self.root.first_pane()
    }

    pub(super) fn place(&mut self, sidebar: &str, width: u16) -> Result<()> {
        #[cfg(unix)]
        {
            if let Err(error) = self.rebuild(sidebar, width) {
                // Recollect live panes before rebuilding again: a failed move
                // may have left some in the staging tab and some in this tab.
                if let Err(recovery) = self.rebuild(sidebar, width) {
                    bail!(
                        "사이드바 재배치 실패: {error:#}. 복구 실패: {recovery:#}. 실행 중인 패널은 닫지 않았습니다. 작업 공간의 'Git Graph layout' 임시 탭을 확인하세요."
                    );
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        bail!("전체 높이 사이드바는 Unix Herdr 소켓이 필요합니다.")
    }

    #[cfg(unix)]
    fn move_to(&mut self, pane: &str, destination: Value) -> Result<Value> {
        let expected_tab = destination["tab_id"].as_str().map(str::to_owned);
        let response = self.socket.call(
            "pane.move",
            json!({"pane_id": pane, "destination": destination, "focus": false}),
        )?;
        ensure!(
            response["move_result"]["changed"] == true,
            "패널 이동이 거부되었습니다. 패널 확대 상태를 확인하세요."
        );
        let moved = &response["move_result"]["pane"];
        ensure!(
            moved["pane_id"] == pane && moved["workspace_id"] == self.workspace,
            "Herdr 패널 이동 중 ID가 변경되었습니다."
        );
        if let Some(tab) = expected_tab {
            ensure!(
                moved["tab_id"] == tab,
                "패널 이동이 거부되었습니다. 패널 확대 상태를 확인하세요."
            );
        }
        Ok(moved.clone())
    }

    #[cfg(unix)]
    fn new_staging_tab(&mut self, pane: &str) -> Result<String> {
        let moved = self.move_to(
            pane,
            json!({"type": "new_tab", "label": "Git Graph layout"}),
        )?;
        Ok(string(&moved, "tab_id")?.into())
    }

    #[cfg(unix)]
    fn insert(
        &mut self,
        pane: &str,
        tab: &str,
        target: &str,
        direction: &str,
        ratio: f64,
    ) -> Result<()> {
        self.move_to(pane, json!({"type": "tab", "tab_id": tab, "target_pane_id": target, "split": direction, "ratio": ratio}))?;
        Ok(())
    }

    #[cfg(unix)]
    fn restore_nodes(&mut self, node: &Node) -> Result<()> {
        if let Node::Split {
            direction,
            ratio,
            first,
            second,
        } = node
        {
            self.insert(
                second.first_pane(),
                &self.tab.clone(),
                first.first_pane(),
                direction,
                *ratio,
            )?;
            self.restore_nodes(first)?;
            self.restore_nodes(second)?;
        }
        Ok(())
    }

    #[cfg(unix)]
    fn rebuild(&mut self, sidebar: &str, width: u16) -> Result<()> {
        let root = self.root.clone();
        let anchor = root.first_pane();
        let mut panes = Vec::new();
        root.panes(&mut panes);
        if let Some(&staging_anchor) = panes.get(1) {
            let staging_tab = self.new_staging_tab(staging_anchor)?;
            for pane in &panes[2..] {
                self.insert(pane, &staging_tab, staging_anchor, "right", 0.5)?;
            }
        }
        let current = self
            .socket
            .call("layout.export", json!({"tab_id": self.tab}))?;
        let current = Node::parse(&current["layout"]["root"])?;
        let correct_pair = match current {
            Node::Split {
                direction: "right",
                first,
                second,
                ..
            } => {
                matches!(first.as_ref(), Node::Pane(id) if id == anchor)
                    && matches!(second.as_ref(), Node::Pane(id) if id == sidebar)
            }
            _ => false,
        };
        if !correct_pair {
            self.new_staging_tab(sidebar)?;
            self.insert(sidebar, &self.tab.clone(), anchor, "right", 0.5)?;
        }
        let snapshot = self
            .socket
            .call("pane.layout", json!({"pane_id": sidebar}))?;
        let available = snapshot["layout"]["area"]["width"]
            .as_u64()
            .filter(|w| *w > 0)
            .context("탭 너비가 없습니다.")?;
        let ratio = (1.0 - f64::from(width) / available as f64).clamp(0.1, 0.9);
        self.socket.call(
            "layout.set_split_ratio",
            json!({"tab_id": self.tab, "path": [], "ratio": ratio}),
        )?;
        self.restore_nodes(&root)?;
        // No-focus moves can temporarily select the anchor when the focused
        // pane is staged. Do not steal focus if the user switched tabs meanwhile.
        let workspace = self
            .socket
            .call("workspace.get", json!({"workspace_id": self.workspace}))?;
        if workspace["workspace"]["focused"] == true
            && workspace["workspace"]["active_tab_id"] == self.tab
        {
            self.socket
                .call("pane.focus", json!({"pane_id": self.focused}))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_sidebar_preserves_the_work_tree_and_ratios() {
        let work = Node::parse(&json!({"type":"split","direction":"down","ratio":0.3,
            "first":{"type":"pane","pane_id":"a"},"second":{"type":"pane","pane_id":"b"}}))
        .unwrap();
        let with_sidebar = Node::Split {
            direction: "right",
            ratio: 0.7,
            first: Box::new(work.clone()),
            second: Box::new(Node::Pane("graph".into())),
        };
        assert_eq!(with_sidebar.without("graph"), Some(work));
        assert!(Node::Pane("graph".into()).without("graph").is_none());
    }

    #[test]
    fn only_a_full_height_right_edge_sidebar_is_already_placed() {
        let layout = json!({"panes":[
            {"pane_id":"a","rect":{"x":0,"y":0,"width":60,"height":20}},
            {"pane_id":"b","rect":{"x":0,"y":20,"width":60,"height":20}},
            {"pane_id":"graph","rect":{"x":60,"y":0,"width":40,"height":40}}]});
        assert!(is_outer_sidebar(&layout, "graph"));
        let mut nested = layout.clone();
        nested["panes"][2]["rect"]["height"] = json!(20);
        assert!(!is_outer_sidebar(&nested, "graph"));
        assert!(!is_outer_sidebar(&layout, "missing"));
        assert!(!is_outer_sidebar(&json!({}), "graph"));
    }
}
