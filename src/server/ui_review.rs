//! UI execution and result shaping behind the unchanged MCP tool signature.

use super::{Kritic, UiParams, finished, started};
use crate::tools::audit;
use crate::ui::{UiOptions, UiResult};
use rmcp::Json;

impl Kritic {
    pub(super) async fn execute_ui_test(&self, p: UiParams) -> Result<Json<UiResult>, String> {
        let t0 = started("ui_test");
        let cfg = &self.kev.cfg;
        let persist: Vec<serde_json::Value> = p
            .persist
            .iter()
            .map(|c| serde_json::json!({"selector": c.selector, "url": c.url}))
            .collect();
        let ttl = cfg.ui_timeout_s.saturating_add(60).clamp(1, 3600);
        let mut r = crate::model_memory::with_reservation(cfg, "kritic-ui-test", ttl, async {
            Ok(crate::ui::ui_test(
                cfg,
                &p.url,
                &p.routes,
                &persist,
                p.max_pages,
                p.max_actions,
                p.allow_destructive,
                p.allow_remote,
                p.headed,
                p.axe,
                &p.flows,
                p.explore,
                UiOptions {
                    axe_blocking: p.axe_blocking,
                    ignore: p.ignore,
                },
            )
            .await)
        })
        .await?;
        r.full_result = crate::tools::save_full(cfg, "ui_test", &r);
        r.report
            .interactions
            .retain(|i| !matches!(i.result.as_str(), "ok" | "navigated"));
        r.report.interactions.truncate(crate::tools::MAX_ITEMS);
        r.report.console_errors.truncate(crate::tools::MAX_ITEMS);
        r.report.failed_requests.truncate(crate::tools::MAX_ITEMS);
        finished("ui_test", t0, &r);
        audit(cfg, "ui_test", &r);
        Ok(Json(r))
    }
}
