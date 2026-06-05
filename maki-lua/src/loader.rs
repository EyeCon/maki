use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use include_dir::{Dir, include_dir};
use maki_agent::tools::ToolRegistry;
use maki_config::{PluginsConfig, RawConfig};

use crate::api::command::{LuaCommandReader, UiAction};
use crate::error::PluginError;
use crate::plugin_permissions::{PluginPermissions, load_plugin_permissions};
use crate::runtime::{self, LuaThread, Request, RestoreItem, RestoreReply};
use maki_agent::prompt::ResolvedSlots;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

struct BundledPlugin {
    name: &'static str,
    dir: Dir<'static>,
}

/// `lib` is not a default builtin; it only exists so plugins can
/// `require()` shared modules across plugin boundaries.
static BUNDLED_PLUGINS: &[BundledPlugin] = &[
    BundledPlugin {
        name: "index",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/index"),
    },
    BundledPlugin {
        name: "webfetch",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/webfetch"),
    },
    BundledPlugin {
        name: "websearch",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/websearch"),
    },
    BundledPlugin {
        name: "bash",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/bash"),
    },
    BundledPlugin {
        name: "glob",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/glob"),
    },
    BundledPlugin {
        name: "skill",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/skill"),
    },
    BundledPlugin {
        name: "memory",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/memory"),
    },
    BundledPlugin {
        name: "question",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/question"),
    },
    BundledPlugin {
        name: "batch",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/batch"),
    },
    BundledPlugin {
        name: "lib",
        dir: include_dir!("$CARGO_MANIFEST_DIR/../plugins/lib"),
    },
];

static BUNDLED_DIRS: LazyLock<&'static [&'static Dir<'static>]> = LazyLock::new(|| {
    let dirs: Vec<&'static Dir<'static>> = BUNDLED_PLUGINS.iter().map(|p| &p.dir).collect();
    Vec::leak(dirs)
});

pub struct PluginHost {
    inner: Option<LuaThread>,
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        let Some(ref mut inner) = self.inner else {
            return;
        };
        let Some(handle) = inner.join.take() else {
            return;
        };
        inner.shutdown.store(true, Ordering::Release);
        let _ = inner.tx.send(Request::Shutdown);
        let (done_tx, done_rx) = flume::bounded(1);
        std::thread::spawn(move || {
            let _ = done_tx.send(handle.join().is_err());
        });
        match done_rx.recv_timeout(SHUTDOWN_TIMEOUT) {
            Ok(true) => tracing::warn!("lua thread panicked on shutdown"),
            Err(_) => tracing::warn!("lua thread did not stop within timeout, detaching"),
            Ok(false) => {}
        }
    }
}

impl PluginHost {
    pub fn new(registry: Arc<ToolRegistry>) -> Result<Self, PluginError> {
        let lua = runtime::spawn(registry, *BUNDLED_DIRS)?;
        Ok(Self { inner: Some(lua) })
    }

    pub fn disabled() -> Self {
        Self { inner: None }
    }

    pub fn load_init_files(&self, cwd: &Path) -> Result<Option<RawConfig>, PluginError> {
        let mut merged: Option<RawConfig> = None;

        for global_dir in maki_config::global_config_dirs() {
            self.run_init_file(&global_dir.join("init.lua"), "global/init.lua", &mut merged)?;
            if merged.is_some() {
                break;
            }
        }
        self.run_init_file(&cwd.join(".maki/init.lua"), "project/init.lua", &mut merged)?;

        Ok(merged)
    }

    fn run_init_file(
        &self,
        path: &Path,
        label: &str,
        merged: &mut Option<RawConfig>,
    ) -> Result<(), PluginError> {
        if !path.is_file() {
            return Ok(());
        }
        let source = fs::read_to_string(path).map_err(|e| PluginError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let plugin_dir = path.parent().map(Path::to_path_buf);
        if let Some(raw) = self.send_run_init_lua(source, label.to_owned(), plugin_dir)? {
            match merged {
                Some(existing) => existing.merge(raw),
                None => *merged = Some(raw),
            }
        }
        Ok(())
    }

    pub fn load_builtins(&mut self, config: &PluginsConfig) -> Result<(), PluginError> {
        for builtin in &config.tools {
            let dir = match BUNDLED_PLUGINS.iter().find(|p| p.name == builtin.as_str()) {
                Some(p) => &p.dir,
                None => {
                    tracing::warn!(
                        builtin = builtin.as_str(),
                        "unknown builtin plugin, skipping"
                    );
                    continue;
                }
            };
            let init = dir
                .get_file("init.lua")
                .and_then(|f| f.contents_utf8())
                .ok_or_else(|| PluginError::Lua {
                    plugin: builtin.clone(),
                    source: mlua::Error::runtime("bundled plugin missing init.lua"),
                })?;
            let name: Arc<str> = Arc::from(builtin.as_str());
            self.send_load(name, init.to_owned(), None, PluginPermissions::trusted())?;
        }
        Ok(())
    }

    fn tx(&self) -> Result<&flume::Sender<Request>, PluginError> {
        self.inner
            .as_ref()
            .map(|r| &r.tx)
            .ok_or(PluginError::HostDead)
    }

    fn send_load(
        &self,
        name: Arc<str>,
        source: String,
        plugin_dir: Option<PathBuf>,
        permissions: PluginPermissions,
    ) -> Result<(), PluginError> {
        let tx = self.tx()?;
        let (reply_tx, reply_rx) = flume::bounded(1);
        tx.send(Request::LoadSource {
            name,
            source,
            plugin_dir,
            permissions,
            reply: reply_tx,
        })
        .map_err(|_| PluginError::HostDead)?;
        reply_rx.recv().map_err(|_| PluginError::HostDead)?
    }

    pub fn send_run_init_lua(
        &self,
        source: String,
        source_name: String,
        plugin_dir: Option<PathBuf>,
    ) -> Result<Option<RawConfig>, PluginError> {
        let tx = self.tx()?;
        let (reply_tx, reply_rx) = flume::bounded(1);
        tx.send(Request::RunInitLua {
            source,
            source_name,
            plugin_dir,
            reply: reply_tx,
        })
        .map_err(|_| PluginError::HostDead)?;
        reply_rx.recv().map_err(|_| PluginError::HostDead)?
    }

    pub fn unload(&self, plugin: &str) -> Result<(), PluginError> {
        let tx = self.tx()?;
        let (reply_tx, reply_rx) = flume::bounded(1);
        tx.send(Request::ClearPlugin {
            plugin: Arc::from(plugin),
            reply: reply_tx,
        })
        .map_err(|_| PluginError::HostDead)?;
        reply_rx.recv().map_err(|_| PluginError::HostDead)?;
        Ok(())
    }

    pub fn load_source(&self, name: &str, source: &str) -> Result<(), PluginError> {
        self.send_load(
            Arc::from(name),
            source.to_owned(),
            None,
            PluginPermissions::trusted(),
        )
    }

    pub fn load_source_with_permissions(
        &self,
        name: &str,
        source: &str,
        permissions: PluginPermissions,
    ) -> Result<(), PluginError> {
        self.send_load(Arc::from(name), source.to_owned(), None, permissions)
    }

    pub fn load_plugin_file(&self, path: &Path) -> Result<(), PluginError> {
        let source = fs::read_to_string(path).map_err(|e| PluginError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let plugin_dir = path.parent().map(Path::to_path_buf);
        let permissions = load_plugin_permissions(plugin_dir.as_deref());
        self.send_load(Arc::from("user"), source, plugin_dir, permissions)
    }

    pub fn event_handle(&self) -> Option<EventHandle> {
        self.inner
            .as_ref()
            .map(|t| EventHandle { tx: t.tx.clone() })
    }

    pub fn command_reader(&self) -> LuaCommandReader {
        self.inner
            .as_ref()
            .map(|t| t.command_reader.clone())
            .unwrap_or_else(LuaCommandReader::empty)
    }

    pub fn ui_action_rx(&self) -> Option<flume::Receiver<UiAction>> {
        self.inner.as_ref().map(|t| t.ui_action_rx.clone())
    }
}

#[derive(Clone)]
pub struct EventHandle {
    tx: flume::Sender<Request>,
}

impl EventHandle {
    #[doc(hidden)]
    pub fn disconnected_for_test() -> Self {
        let (tx, _rx) = flume::unbounded();
        Self { tx }
    }
    pub fn fire_click(&self, tool_id: &str, row: u32) {
        let _ = self.tx.try_send(Request::FireBufClick {
            tool_id: tool_id.to_owned(),
            row,
        });
    }

    pub fn run_command(&self, plugin: Arc<str>, command: Arc<str>, args: String) {
        let _ = self.tx.try_send(Request::RunCommand {
            plugin,
            command,
            args,
        });
    }

    pub fn collect_prompt_slots(&self) -> ResolvedSlots {
        let (tx, rx) = flume::bounded(1);
        let _ = self.tx.send(Request::CollectPromptSlots { reply: tx });
        rx.recv().unwrap_or_default()
    }

    pub async fn collect_prompt_slots_async(&self) -> ResolvedSlots {
        let (tx, rx) = flume::bounded(1);
        let _ = self.tx.send(Request::CollectPromptSlots { reply: tx });
        rx.recv_async().await.unwrap_or_default()
    }

    /// One round-trip for N tools instead of N blocking channel hops.
    pub fn restore_tool_batch(&self, items: Vec<RestoreItem>) -> Vec<Option<RestoreReply>> {
        let (tx, rx) = flume::bounded(1);
        let _ = self.tx.send(Request::RestoreToolBatch { items, reply: tx });
        rx.recv().unwrap_or_default()
    }

    /// Non-blocking variant: result flows back through `event_tx` so theme
    /// re-bakes don't block the UI thread.
    pub fn request_restore(&self, item: RestoreItem, event_tx: maki_agent::EventSender) {
        let _ = self.tx.send(Request::RestoreToolAsync { item, event_tx });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::command::{LuaCommandInfo, LuaCommandWriter};
    use maki_agent::prompt::{PromptId, ResolvedSlots, Slot};
    use maki_agent::tools::ToolRegistry;
    use test_case::test_case;

    /// Load `src` as a single plugin and collect the resolved slots. Panics on
    /// load failure; reach for `load_err` when you want to inspect the error.
    fn slots_from(plugin: &str, src: &str) -> (PluginHost, ResolvedSlots) {
        let host = PluginHost::new(Arc::new(ToolRegistry::new())).unwrap();
        host.load_source(plugin, src).unwrap();
        let slots = host.event_handle().unwrap().collect_prompt_slots();
        (host, slots)
    }

    fn contents(slots: &ResolvedSlots, prompt: PromptId, slot: Slot) -> Vec<&str> {
        slots
            .get(prompt, slot)
            .iter()
            .map(|e| e.content.as_str())
            .collect()
    }

    #[test]
    fn command_writer_reader_pair_works() {
        let (writer, reader) = LuaCommandWriter::new();
        let snap = reader.load();
        assert_eq!(snap.commands.len(), 0);

        writer.publish(vec![LuaCommandInfo {
            name: Arc::from("/test"),
            description: Arc::from("desc"),
            plugin: Arc::from("p"),
        }]);
        let snap = reader.load();
        assert_eq!(snap.commands.len(), 1);
        assert!(snap.generation > 0);
    }

    #[test]
    fn memory_builtin_registers_command() {
        let reg = Arc::new(ToolRegistry::new());
        let mut host = PluginHost::new(Arc::clone(&reg)).unwrap();
        host.load_builtins(&PluginsConfig::from_tools(std::collections::HashMap::new()))
            .unwrap();
        let reader = host.command_reader();
        let snap = reader.load();
        let found = snap.commands.iter().any(|c| c.name.as_ref() == "/memory");
        assert!(
            found,
            "Expected /memory command, found: {:?}",
            snap.commands
                .iter()
                .map(|c| c.name.as_ref())
                .collect::<Vec<_>>()
        );
    }

    const BATCH_BUILTIN_MSG: &str = "batch must be loaded as a builtin plugin";

    #[test]
    fn batch_builtin_registers_tool() {
        let reg = Arc::new(ToolRegistry::new());
        let mut host = PluginHost::new(Arc::clone(&reg)).unwrap();
        host.load_builtins(&PluginsConfig::from_tools(std::collections::HashMap::new()))
            .unwrap();
        assert!(reg.get("batch").is_some(), "{BATCH_BUILTIN_MSG}");
    }

    #[test]
    fn run_command_sends_correct_request() {
        let (tx, rx) = flume::bounded(8);
        let handle = EventHandle { tx };
        handle.run_command(Arc::from("myplugin"), Arc::from("/greet"), "world".into());
        let req = rx.try_recv().unwrap();
        match req {
            Request::RunCommand {
                plugin,
                command,
                args,
            } => {
                assert_eq!(plugin.as_ref(), "myplugin");
                assert_eq!(command.as_ref(), "/greet");
                assert_eq!(args, "world");
            }
            _ => panic!("expected RunCommand"),
        }
    }

    #[test]
    fn multiple_plugins_register_independent_commands() {
        let reg = Arc::new(ToolRegistry::new());
        let host = PluginHost::new(Arc::clone(&reg)).unwrap();
        host.load_source(
            "plugin_a",
            r#"
            maki.api.register_command({
                name = "/alpha",
                description = "from a",
                handler = function() end,
            })
            "#,
        )
        .unwrap();
        host.load_source(
            "plugin_b",
            r#"
            maki.api.register_command({
                name = "/beta",
                description = "from b",
                handler = function() end,
            })
            "#,
        )
        .unwrap();

        let snap = host.command_reader().load();
        assert_eq!(snap.commands.len(), 2);
        let names: Vec<&str> = snap.commands.iter().map(|c| c.name.as_ref()).collect();
        assert!(names.contains(&"/alpha"));
        assert!(names.contains(&"/beta"));
    }

    #[test]
    fn command_reader_generation_increments_on_publish() {
        let (writer, reader) = LuaCommandWriter::new();
        assert_eq!(reader.load().generation, 0);
        writer.publish(vec![]);
        assert!(reader.load().generation > 0);
    }

    #[test]
    fn disabled_host_returns_defaults() {
        let host = PluginHost::disabled();
        let snap = host.command_reader().load();
        assert_eq!(snap.commands.len(), 0);
        assert_eq!(snap.generation, 0);
        assert!(host.ui_action_rx().is_none());
    }

    #[test]
    fn callback_string_lands_in_targeted_prompt_only() {
        let (_host, slots) = slots_from(
            "cb",
            r#"
            maki.api.register_prompt_hint({
                slot = "tool_usage",
                prompt = "general",
                content = function() return "from_cb" end,
            })
            "#,
        );
        assert_eq!(
            contents(&slots, PromptId::General, Slot::ToolUsage),
            ["from_cb"]
        );
        assert!(contents(&slots, PromptId::System, Slot::ToolUsage).is_empty());
    }

    #[test]
    fn callback_returning_nil_contributes_nothing() {
        let (_host, slots) = slots_from(
            "nil_cb",
            r#"
            maki.api.register_prompt_hint({
                slot = "tool_usage",
                content = function() return nil end,
            })
            "#,
        );
        assert!(contents(&slots, PromptId::System, Slot::ToolUsage).is_empty());
    }

    /// A hint with no `prompt` is a default: it lands on every prompt that has the slot.
    #[test]
    fn static_no_prompt_lands_on_all_prompts_with_slot() {
        let (_host, slots) = slots_from(
            "static_hint",
            r#"
            maki.api.register_prompt_hint({
                slot = "efficient_tools",
                content = "index",
            })
            "#,
        );
        for &pid in PromptId::ALL {
            assert_eq!(contents(&slots, pid, Slot::EfficientTools), ["index"]);
        }
    }

    /// `conventions` lives on system and general but not research, so a default
    /// hint follows the slot and skips research.
    #[test]
    fn default_hint_skips_prompts_lacking_the_slot() {
        let (_host, slots) = slots_from(
            "conv",
            r#"
            maki.api.register_prompt_hint({
                slot = "conventions",
                content = "follow conventions",
            })
            "#,
        );
        for pid in [PromptId::System, PromptId::General] {
            assert_eq!(
                contents(&slots, pid, Slot::Conventions),
                ["follow conventions"]
            );
        }
        assert!(contents(&slots, PromptId::Research, Slot::Conventions).is_empty());
    }

    /// Targeting a prompt that does not have the slot quietly drops the hint.
    #[test]
    fn explicit_prompt_without_slot_is_dropped() {
        let (_host, slots) = slots_from(
            "drop",
            r#"
            maki.api.register_prompt_hint({
                slot = "after_instructions",
                prompt = "research",
                content = "never lands",
            })
            "#,
        );
        assert!(contents(&slots, PromptId::Research, Slot::AfterInstructions).is_empty());
    }

    #[test]
    fn prompt_list_targets_each_listed_prompt() {
        const CONTENT: &str = "shared";
        let (_host, slots) = slots_from(
            "list",
            r#"
            maki.api.register_prompt_hint({
                slot = "tool_usage",
                prompt = { "system", "research" },
                content = "shared",
            })
            "#,
        );
        assert_eq!(
            contents(&slots, PromptId::System, Slot::ToolUsage),
            [CONTENT]
        );
        assert_eq!(
            contents(&slots, PromptId::Research, Slot::ToolUsage),
            [CONTENT]
        );
        assert!(contents(&slots, PromptId::General, Slot::ToolUsage).is_empty());
    }

    #[test]
    fn multiple_plugins_sorted_by_plugin_name() {
        let host = PluginHost::new(Arc::new(ToolRegistry::new())).unwrap();
        for plugin in ["zzz", "aaa"] {
            host.load_source(
                plugin,
                r#"
                maki.api.register_prompt_hint({ slot = "tool_usage", content = "from_PLUGIN" })
                "#
                .replace("PLUGIN", plugin)
                .as_str(),
            )
            .unwrap();
        }
        let slots = host.event_handle().unwrap().collect_prompt_slots();
        assert_eq!(
            contents(&slots, PromptId::System, Slot::ToolUsage),
            ["from_aaa", "from_zzz"],
            "entries must be ordered by plugin name"
        );
    }

    /// One plugin can register several hints; unloading it clears all of them.
    #[test]
    fn unload_clears_all_hints_from_plugin() {
        let host = PluginHost::new(Arc::new(ToolRegistry::new())).unwrap();
        host.load_source(
            "multi",
            r#"
            maki.api.register_prompt_hint({ slot = "tool_usage", prompt = "system", content = "usage" })
            maki.api.register_prompt_hint({ slot = "conventions", prompt = "system", content = "conv" })
            "#,
        )
        .unwrap();
        let handle = host.event_handle().unwrap();

        let slots = handle.collect_prompt_slots();
        assert_eq!(
            contents(&slots, PromptId::System, Slot::ToolUsage),
            ["usage"]
        );
        assert_eq!(
            contents(&slots, PromptId::System, Slot::Conventions),
            ["conv"]
        );

        host.unload("multi").unwrap();
        let slots = handle.collect_prompt_slots();
        assert!(contents(&slots, PromptId::System, Slot::ToolUsage).is_empty());
        assert!(contents(&slots, PromptId::System, Slot::Conventions).is_empty());
    }

    #[test_case(r#"{ slot = "nonexistent", content = "x" }"# ; "invalid_slot")]
    #[test_case(r#"{ slot = "tool_usage", content = "x", prompt = "nope" }"# ; "invalid_prompt")]
    #[test_case(r#"{ slot = "tool_usage", content = "x", prompt = { "system", "bogus" } }"# ; "invalid_prompt_in_list")]
    #[test_case(r#"{ slot = "tool_usage" }"# ; "missing_content")]
    #[test_case(r#"{ content = "x" }"# ; "missing_slot")]
    #[test_case(r#"{ slot = "tool_usage", content = 42 }"# ; "content_wrong_type")]
    #[test_case(r#"{ slot = "tool_usage", content = "x", prompt = 42 }"# ; "prompt_wrong_type")]
    fn invalid_hint_spec_is_rejected(spec: &str) {
        let host = PluginHost::new(Arc::new(ToolRegistry::new())).unwrap();
        let src = format!("maki.api.register_prompt_hint({spec})");
        assert!(host.load_source("bad", &src).is_err());
    }

    const LIVE_BUF_MSG: &str = "inner tool must emit LiveToolBuf for child tool";

    #[test]
    fn call_tool_emits_live_tool_buf_for_inner_tool() {
        use maki_agent::agent::tool_dispatch::{self, Emit};
        use maki_agent::tools::test_support::stub_ctx_with;
        use maki_agent::{AgentEvent, AgentMode, Envelope, EventSender};

        let reg = ToolRegistry::native_arc();
        let host = PluginHost::new(Arc::clone(reg)).unwrap();

        host.load_source(
            "inner",
            r#"
            maki.api.register_tool({
                name = "inner_tool",
                description = "test inner",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local buf = maki.ui.buf()
                    buf:line("live line 1")
                    buf:line("live line 2")
                    ctx:finish({ llm_output = "inner done", body = buf })
                    return nil
                end,
            })
            "#,
        )
        .unwrap();

        host.load_source(
            "outer",
            r#"
            maki.api.register_tool({
                name = "outer_tool",
                description = "test outer",
                schema = {
                    type = "object",
                    properties = {},
                },
                handler = function(input, ctx)
                    local reply = maki.api.call_tool("inner_tool", {}, { id = "outer1__0" })
                    return reply.output
                end,
            })
            "#,
        )
        .unwrap();

        smol::block_on(async {
            let (tx, rx) = flume::unbounded::<Envelope>();
            let event_tx = EventSender::new(tx, 0);
            let ctx = stub_ctx_with(&AgentMode::Build, Some(&event_tx), Some("outer1"));

            let done = tool_dispatch::run(
                reg,
                None,
                "outer1".into(),
                "outer_tool",
                &serde_json::json!({}),
                &ctx,
                Emit::Notify,
            )
            .await;

            assert!(
                !done.is_error,
                "outer_tool failed: {}",
                done.output.as_text()
            );

            let mut found_live_buf = false;
            let mut found_child_done = false;
            while let Ok(env) = rx.try_recv() {
                match env.event {
                    AgentEvent::LiveToolBuf { id, body } if id == "outer1__0" => {
                        found_live_buf = true;
                        assert!(!body.is_empty(), "LiveToolBuf body should have content");
                    }
                    AgentEvent::ToolDone(e) if e.id == "outer1__0" => {
                        found_child_done = true;
                    }
                    _ => {}
                }
            }
            assert!(found_live_buf, "{LIVE_BUF_MSG}");
            assert!(found_child_done, "inner tool must emit ToolDone");
        });
    }

    #[test]
    fn call_tool_streams_jobstart_output_via_live_buf() {
        use maki_agent::agent::tool_dispatch::{self, Emit};
        use maki_agent::tools::test_support::stub_ctx_with;
        use maki_agent::{AgentEvent, AgentMode, Envelope, EventSender};

        let reg = ToolRegistry::native_arc();
        let host = PluginHost::new(Arc::clone(reg)).unwrap();

        host.load_source(
            "inner_job",
            r#"
            maki.api.register_tool({
                name = "inner_job_tool",
                description = "uses jobstart",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local buf = maki.ui.buf()
                    buf:line("waiting")

                    maki.fn.jobstart("printf 'line1\nline2\nline3\n'", {
                        on_stdout = function(_, line)
                            buf:line(line)
                        end,
                        on_exit = function(_, code)
                            ctx:finish({ llm_output = "done", body = buf })
                        end,
                    })
                    return nil
                end,
            })
            "#,
        )
        .unwrap();

        host.load_source(
            "outer_job",
            r#"
            maki.api.register_tool({
                name = "outer_job_tool",
                description = "calls inner via call_tool",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local reply = maki.api.call_tool("inner_job_tool", {}, { id = "oj1__0" })
                    return reply.output
                end,
            })
            "#,
        )
        .unwrap();

        smol::block_on(async {
            let (tx, rx) = flume::unbounded::<Envelope>();
            let event_tx = EventSender::new(tx, 0);
            let ctx = stub_ctx_with(&AgentMode::Build, Some(&event_tx), Some("oj1"));

            let done = tool_dispatch::run(
                reg,
                None,
                "oj1".into(),
                "outer_job_tool",
                &serde_json::json!({}),
                &ctx,
                Emit::Notify,
            )
            .await;

            assert!(
                !done.is_error,
                "outer_job_tool failed: {}",
                done.output.as_text()
            );

            let mut live_buf_received_before_done = false;
            let mut live_buf_body = None;
            let mut child_done = false;

            while let Ok(env) = rx.try_recv() {
                match env.event {
                    AgentEvent::LiveToolBuf { id, body } if id == "oj1__0" => {
                        if !child_done {
                            live_buf_received_before_done = true;
                        }
                        live_buf_body = Some(body);
                    }
                    AgentEvent::ToolDone(e) if e.id == "oj1__0" => {
                        child_done = true;
                    }
                    _ => {}
                }
            }

            assert!(
                live_buf_body.is_some(),
                "LiveToolBuf must be emitted for inner jobstart tool"
            );
            assert!(
                live_buf_received_before_done,
                "LiveToolBuf must arrive before ToolDone for streaming"
            );
            let buf = live_buf_body.unwrap();
            assert!(
                !buf.is_empty(),
                "SharedBuf must have content after tool completes"
            );
        });
    }

    const BATCH_HANG_MSG: &str = "async.join with call_tool must complete without hanging";

    #[test]
    fn async_join_with_call_tool_completes() {
        use maki_agent::agent::tool_dispatch::{self, Emit};
        use maki_agent::tools::test_support::stub_ctx_with;
        use maki_agent::{AgentMode, Envelope, EventSender};

        let reg = ToolRegistry::native_arc();
        let host = PluginHost::new(Arc::clone(reg)).unwrap();

        host.load_source(
            "echo_tool_src",
            r#"
            maki.api.register_tool({
                name = "echo_tool",
                description = "echoes input",
                schema = { type = "object", properties = { msg = { type = "string" } } },
                handler = function(input, ctx)
                    return input.msg or "no msg"
                end,
            })
            "#,
        )
        .unwrap();

        host.load_source(
            "batch_like",
            r#"
            maki.api.register_tool({
                name = "batch_like_tool",
                description = "uses async.join + call_tool like batch",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local results = {}
                    local funs = {}
                    local buf = maki.ui.buf()
                    for i = 1, 3 do
                        funs[i] = function()
                            local reply = maki.api.call_tool("echo_tool", { msg = "hello " .. i }, {
                                id = "bl1__" .. (i - 1),
                                parent_id = "bl1",
                                on_output = function(child_buf)
                                    results[i] = { body = child_buf }
                                    buf:set_lines({})
                                    buf:line("updated " .. i)
                                end,
                            })
                            results[i] = reply
                        end
                    end
                    maki.async.join(3, funs)
                    local parts = {}
                    for i = 1, 3 do
                        parts[i] = results[i] and results[i].output or "nil"
                    end
                    return { llm_output = table.concat(parts, "|"), body = buf }
                end,
            })
            "#,
        )
        .unwrap();

        smol::block_on(async {
            let (tx, _rx) = flume::unbounded::<Envelope>();
            let event_tx = EventSender::new(tx, 0);
            let ctx = stub_ctx_with(&AgentMode::Build, Some(&event_tx), Some("bl1"));

            let done = smol::future::or(
                async {
                    tool_dispatch::run(
                        reg,
                        None,
                        "bl1".into(),
                        "batch_like_tool",
                        &serde_json::json!({}),
                        &ctx,
                        Emit::Notify,
                    )
                    .await
                },
                async {
                    smol::Timer::after(Duration::from_secs(10)).await;
                    panic!("{BATCH_HANG_MSG}");
                },
            )
            .await;

            assert!(
                !done.is_error,
                "batch_like_tool failed: {}",
                done.output.as_text()
            );
            let text = done.output.as_text();
            assert!(
                text.contains("hello 1") && text.contains("hello 2") && text.contains("hello 3"),
                "expected all 3 results, got: {text}"
            );
        });
    }

    const STREAMING_MSG: &str = "SharedBuf must be dirty while subprocess is running";

    /// Children that yield mid-handler make their `TaskScope`s interleave on
    /// the Lua executor, so one scope captures another's handle as `prev` and
    /// re-publishes it into app_data on drop. The pinned `TaskCell` must not
    /// keep the child's captured event sender alive, or the dispatch drain
    /// never finishes and `call_tool` hangs (batch stuck in-progress).
    #[test]
    fn async_join_with_yielding_children_completes() {
        use maki_agent::agent::tool_dispatch::{self, Emit};
        use maki_agent::tools::test_support::stub_ctx_with;
        use maki_agent::{AgentMode, Envelope, EventSender};

        let reg = ToolRegistry::native_arc();
        let host = PluginHost::new(Arc::clone(reg)).unwrap();

        host.load_source(
            "sleepy",
            r#"
            maki.api.register_tool({
                name = "sleepy_tool",
                description = "sleeps via jobstart",
                schema = { type = "object", properties = { secs = { type = "string" } } },
                handler = function(input, ctx)
                    local buf = maki.ui.buf()
                    buf:line("waiting " .. input.secs)
                    maki.fn.jobstart("sleep " .. input.secs, {
                        on_exit = function(_, code)
                            ctx:finish({ llm_output = "slept " .. input.secs, body = buf })
                        end,
                    })
                    return nil
                end,
            })
            "#,
        )
        .unwrap();

        host.load_source(
            "batch_slow",
            r#"
            maki.api.register_tool({
                name = "batch_slow_tool",
                description = "joins yielding children like batch",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local results = {}
                    local funs = {}
                    local secs = { "0.1", "0.3" }
                    for i = 1, 2 do
                        funs[i] = function()
                            local reply = maki.api.call_tool("sleepy_tool", { secs = secs[i] }, {
                                id = "sb1__" .. (i - 1),
                                parent_id = "sb1",
                                on_output = function(_) end,
                            })
                            results[i] = reply.output
                        end
                    end
                    maki.async.join(2, funs)
                    return table.concat(results, "|")
                end,
            })
            "#,
        )
        .unwrap();

        smol::block_on(async {
            let (tx, _rx) = flume::unbounded::<Envelope>();
            let event_tx = EventSender::new(tx, 0);
            let ctx = stub_ctx_with(&AgentMode::Build, Some(&event_tx), Some("sb1"));

            let done = smol::future::or(
                async {
                    tool_dispatch::run(
                        reg,
                        None,
                        "sb1".into(),
                        "batch_slow_tool",
                        &serde_json::json!({}),
                        &ctx,
                        Emit::Notify,
                    )
                    .await
                },
                async {
                    smol::Timer::after(Duration::from_secs(10)).await;
                    panic!("{BATCH_HANG_MSG}");
                },
            )
            .await;

            assert!(
                !done.is_error,
                "batch_slow_tool failed: {}",
                done.output.as_text()
            );
            let text = done.output.as_text();
            assert_eq!(text, "slept 0.1|slept 0.3");
        });
    }

    #[test]
    fn call_tool_jobstart_buf_updates_incrementally() {
        use maki_agent::agent::tool_dispatch::{self, Emit};
        use maki_agent::tools::test_support::stub_ctx_with;
        use maki_agent::{AgentEvent, AgentMode, Envelope, EventSender, SharedBuf};

        let reg = ToolRegistry::native_arc();
        let host = PluginHost::new(Arc::clone(reg)).unwrap();

        host.load_source(
            "slow_inner",
            r#"
            maki.api.register_tool({
                name = "slow_inner_tool",
                description = "slow subprocess",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local buf = maki.ui.buf()
                    maki.fn.jobstart("for i in 1 2 3; do echo line_$i; sleep 0.15; done", {
                        on_stdout = function(_, line)
                            buf:line(line)
                        end,
                        on_exit = function(_, code)
                            ctx:finish({ llm_output = "done", body = buf })
                        end,
                    })
                    return nil
                end,
            })
            "#,
        )
        .unwrap();

        host.load_source(
            "slow_outer",
            r#"
            maki.api.register_tool({
                name = "slow_outer_tool",
                description = "calls slow inner",
                schema = { type = "object", properties = {} },
                handler = function(input, ctx)
                    local reply = maki.api.call_tool("slow_inner_tool", {}, { id = "so1__0" })
                    return reply.output
                end,
            })
            "#,
        )
        .unwrap();

        smol::block_on(async {
            let (tx, rx) = flume::unbounded::<Envelope>();
            let event_tx = EventSender::new(tx, 0);
            let ctx = stub_ctx_with(&AgentMode::Build, Some(&event_tx), Some("so1"));

            let run_handle = smol::spawn({
                let reg = Arc::clone(reg);
                let ctx = ctx.clone();
                async move {
                    tool_dispatch::run(
                        &reg,
                        None,
                        "so1".into(),
                        "slow_outer_tool",
                        &serde_json::json!({}),
                        &ctx,
                        Emit::Notify,
                    )
                    .await
                }
            });

            // Wait for LiveToolBuf event
            let mut shared_buf: Option<Arc<SharedBuf>> = None;
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while shared_buf.is_none() && std::time::Instant::now() < deadline {
                if let Ok(env) = rx.try_recv() {
                    if let AgentEvent::LiveToolBuf { id, body } = env.event {
                        if id == "so1__0" {
                            shared_buf = Some(body);
                        }
                    }
                }
                smol::Timer::after(Duration::from_millis(10)).await;
            }
            assert!(shared_buf.is_some(), "LiveToolBuf must be emitted");
            let buf = shared_buf.unwrap();

            // Poll the buf over time and track distinct lengths seen
            let mut seen_lengths = vec![buf.len()];
            for _ in 0..30 {
                smol::Timer::after(Duration::from_millis(50)).await;
                let current_len = buf.len();
                if Some(&current_len) != seen_lengths.last() {
                    seen_lengths.push(current_len);
                }
            }

            let done = run_handle.await;
            assert!(
                !done.is_error,
                "slow_outer_tool failed: {}",
                done.output.as_text()
            );

            assert!(
                seen_lengths.len() >= 2,
                // The subprocess echoes 3 lines with 150ms sleep between.
                // We poll every 50ms, so we should see at least 2 distinct lengths.
                "{STREAMING_MSG}: saw only these lengths: {seen_lengths:?}"
            );
        });
    }
}
