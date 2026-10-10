use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Workspace {
    pub id: u64,
    pub idx: u32,
    pub name: Option<String>,
    pub output: Option<String>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_focused: bool,
    #[serde(default)]
    pub is_urgent: bool,
    pub active_window_id: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Window {
    pub id: u64,
    pub title: Option<String>,
    pub app_id: Option<String>,
    pub workspace_id: Option<u64>,
    #[serde(default)]
    pub is_focused: bool,
    #[serde(default)]
    pub is_urgent: bool,
    #[serde(default)]
    pub layout: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct LogicalOutput {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Output {
    pub name: String,
    pub logical: Option<LogicalOutput>,
}

#[derive(Default, Debug)]
pub struct State {
    pub workspaces: BTreeMap<u64, Workspace>,
    pub windows: BTreeMap<u64, Window>,
    pub outputs: BTreeMap<String, Output>,
    pub output_revision: u64,
    pub overview_open: bool,
    pub connected: bool,
    pub revision: u64,
}

impl State {
    pub fn workspaces_on(&self, output: &str) -> Vec<&Workspace> {
        let mut workspaces: Vec<_> = self
            .workspaces
            .values()
            .filter(|w| w.output.as_deref() == Some(output))
            .collect();
        workspaces.sort_by_key(|w| (w.idx, w.id));
        workspaces
    }

    pub fn active_on(&self, output: &str) -> Option<&Workspace> {
        self.workspaces_on(output).into_iter().find(|w| w.is_active)
    }

    pub fn windows_on(&self, output: &str) -> Vec<&Window> {
        let Some(workspace) = self.active_on(output) else {
            return Vec::new();
        };
        let mut windows: Vec<_> = self
            .windows
            .values()
            .filter(|w| w.workspace_id == Some(workspace.id))
            .collect();
        windows.sort_by(|a, b| {
            let x = |w: &Window| {
                w.layout["pos_in_scrolling_layout"][0]
                    .as_f64()
                    .unwrap_or(0.)
            };
            x(a).total_cmp(&x(b)).then(a.id.cmp(&b.id))
        });
        windows
    }

    pub fn apply(&mut self, event: Value) -> Result<bool, serde_json::Error> {
        let Some((name, data)) = event.as_object().and_then(|e| e.iter().next()) else {
            return Ok(false);
        };
        let id = || data["id"].as_u64().unwrap_or(u64::MAX);
        match name.as_str() {
            "WorkspacesChanged" => {
                let items: Vec<Workspace> = serde_json::from_value(data["workspaces"].clone())?;
                self.workspaces = items.into_iter().map(|w| (w.id, w)).collect();
            }
            "WindowsChanged" => {
                let items: Vec<Window> = serde_json::from_value(data["windows"].clone())?;
                self.windows = items.into_iter().map(|w| (w.id, w)).collect();
            }
            "WorkspaceActivated" => {
                let output = self.workspaces.get(&id()).and_then(|w| w.output.clone());
                let focused = data["focused"].as_bool().unwrap_or(false);
                for w in self.workspaces.values_mut() {
                    if w.id == id() {
                        w.is_active = true;
                        w.is_focused = focused;
                    } else {
                        if output.is_some() && w.output == output {
                            w.is_active = false;
                        }
                        if focused {
                            w.is_focused = false;
                        }
                    }
                }
            }
            "WorkspaceUrgencyChanged" => {
                if let Some(w) = self.workspaces.get_mut(&id()) {
                    w.is_urgent = data["urgent"].as_bool().unwrap_or(false);
                }
            }
            "WorkspaceActiveWindowChanged" => {
                if let Some(w) = data["workspace_id"]
                    .as_u64()
                    .and_then(|id| self.workspaces.get_mut(&id))
                {
                    w.active_window_id = data["active_window_id"].as_u64();
                }
            }
            "WindowOpenedOrChanged" => {
                let window: Window = serde_json::from_value(data["window"].clone())?;
                if window.is_focused {
                    for w in self.windows.values_mut() {
                        w.is_focused = false;
                    }
                }
                self.windows.insert(window.id, window);
            }
            "WindowClosed" => {
                self.windows.remove(&id());
            }
            "WindowFocusChanged" => {
                for w in self.windows.values_mut() {
                    w.is_focused = Some(w.id) == data["id"].as_u64();
                }
            }
            "WindowUrgencyChanged" => {
                if let Some(w) = self.windows.get_mut(&id()) {
                    w.is_urgent = data["urgent"].as_bool().unwrap_or(false);
                }
            }
            "WindowLayoutsChanged" => {
                if let Some(changes) = data["changes"].as_array() {
                    for change in changes {
                        if let Some(w) = change[0].as_u64().and_then(|id| self.windows.get_mut(&id))
                        {
                            w.layout = change[1].clone();
                        }
                    }
                }
            }
            "OverviewOpenedOrClosed" => {
                self.overview_open = data["is_open"].as_bool().unwrap_or(false);
            }
            _ => return Ok(false),
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn invalid_snapshots_preserve_state_and_unknown_events_are_ignored() {
        let mut state = State::default();
        state.workspaces.insert(
            42,
            Workspace {
                id: 42,
                ..Default::default()
            },
        );
        assert!(
            state
                .apply(json!({"WorkspacesChanged":{"workspaces":[{"id":"bad"}]}}))
                .is_err()
        );
        assert!(state.workspaces.contains_key(&42));
        assert!(
            !state
                .apply(json!({"FutureEvent":{"anything":true}}))
                .unwrap()
        );
        assert_eq!(state.revision, 0);
    }

    #[test]
    fn output_lists_and_activation_are_independent() {
        let mut s = State::default();
        s.apply(json!({"WorkspacesChanged":{"workspaces":[
            {"id":1,"idx":1,"output":"DP-1","is_active":true,"is_focused":true},
            {"id":2,"idx":2,"output":"DP-1"},
            {"id":3,"idx":1,"output":"HDMI-A-1","is_active":true},
            {"id":4,"idx":2,"output":null}
        ]}}))
        .unwrap();
        s.apply(json!({"WorkspaceActivated":{"id":2,"focused":true}}))
            .unwrap();
        assert_eq!(s.workspaces_on("DP-1").len(), 2);
        assert_eq!(s.workspaces_on("HDMI-A-1").len(), 1);
        assert_eq!(s.active_on("HDMI-A-1").unwrap().id, 3);
        assert_eq!(s.active_on("DP-1").unwrap().id, 2);
        assert!(s.workspaces[&2].is_focused);
        assert!(!s.workspaces[&1].is_active);
    }

    #[test]
    fn dock_follows_output_and_column_order() {
        let mut s = State::default();
        s.workspaces.insert(
            1,
            Workspace {
                id: 1,
                output: Some("DP-1".into()),
                is_active: true,
                ..Default::default()
            },
        );
        s.workspaces.insert(
            2,
            Workspace {
                id: 2,
                output: Some("DP-2".into()),
                is_active: true,
                ..Default::default()
            },
        );
        s.apply(json!({"WindowsChanged":{"windows":[
            {"id":1,"workspace_id":1,"layout":{"pos_in_scrolling_layout":[3,0]}},
            {"id":2,"workspace_id":1,"layout":{"pos_in_scrolling_layout":[1,0]}},
            {"id":3,"workspace_id":2}
        ]}}))
        .unwrap();
        assert_eq!(
            s.windows_on("DP-1")
                .iter()
                .map(|w| w.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(s.windows_on("DP-2")[0].id, 3);
        // Niri can move a workspace to another output while keeping its ID.
        s.workspaces.get_mut(&1).unwrap().output = Some("DP-3".into());
        assert!(s.windows_on("DP-1").is_empty());
        assert_eq!(s.windows_on("DP-3").len(), 2);
    }
}
