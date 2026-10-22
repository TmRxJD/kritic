"""Pure policy: fitting-row measurements remain advisory."""


def advisory(why):
    return {"gate": "advisory", "why": why}


def calibrated(key, reason, cases, below=False, *, results, source):
    r = results["results"].get(key)
    if r is None:
        return advisory(f"not calibrated: {key} is missing from logs/calibration-specs.json (run scripts/calibrate-specs.mjs)")
    op = r.get("operatingPoint") or {}
    evidence = (f"{results['date']}, scripts/calibrate-specs.mjs on {source}: "
                f"{r['good']} good vs {r['bad']} bad ({cases}); AUC {r['auc']}; at 5% false rejects "
                f"{round(op.get('caught', 0) * 100)}% caught")
    # This legacy report fits thresholds and reports accuracy on the same rows.
    # Even old reports with promote=true cannot justify a production rule.
    return advisory(f"measured {evidence}: exploratory fitting only; independent grouped held-out "
                    "evaluation with frozen thresholds and final deployed model is required before gating")
