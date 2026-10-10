//! Offline deadline/cancellation contracts; no backend, files, GPU or inference.
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use kritic::config::Config;
use kritic::critic::{Deadline, critic, critic_diff};
use kritic::kev::{Decider, Decision, Question};
use serde_json::Value;

struct DropCounter(Arc<AtomicUsize>);
impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct PendingDecision(Arc<AtomicUsize>);
impl Decider for PendingDecision {
    async fn decide(&self, _: &Value, _: &BTreeMap<String, Question>) -> Result<Decision, String> {
        let _guard = DropCounter(self.0.clone());
        std::future::pending().await
    }
}

#[tokio::test]
async fn pending_decision_is_cancelled_in_both_critic_modes() {
    let mut cfg = Config::load();
    cfg.critic_deadline_s = 1;
    for diff_mode in [false, true] {
        let dropped = Arc::new(AtomicUsize::new(0));
        let decider = PendingDecision(dropped.clone());
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            if diff_mode {
                critic_diff(&decider, &cfg, "change", "diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -0,0 +1 @@\n+let x = 1;\n", 0).await
            } else {
                critic(&decider, &cfg, "change", &[("x.rs".into(), "let x = 1;".into())], 0).await
            }
        }).await.expect("a single in-flight decision must respect the whole-pass deadline");
        assert!(!result.kev_ok);
        assert_eq!(result.approval, None);
        assert!(
            result
                .error
                .unwrap()
                .contains("remote cancellation unconfirmed")
        );
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn phases_share_an_absolute_deadline() {
    let mut cfg = Config::load();
    cfg.critic_deadline_s = 1;
    let deadline = Deadline::new(&cfg);
    deadline
        .bound(
            async {
                tokio::time::sleep(Duration::from_millis(600)).await;
                Ok(())
            },
            0,
            2,
        )
        .await
        .unwrap();
    let dropped = Arc::new(AtomicUsize::new(0));
    let counter = dropped.clone();
    let second = deadline
        .bound(
            async move {
                let _guard = DropCounter(counter);
                tokio::time::sleep(Duration::from_millis(600)).await;
                Ok(())
            },
            1,
            2,
        )
        .await;
    assert!(second.unwrap_err().contains("1/2 units complete"));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
