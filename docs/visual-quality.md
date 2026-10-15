# Visual craft: professional does not mean merely functional

Kritic previously approved screenshots when five baseline scores met a default minimum of 6.
Its prompt called 6–8 "acceptable with issues", and a conventional settings-card fixture served
as a polished example. That did not establish professional typography, art direction or finish.
The revised rubric deliberately retires that aesthetic approval contract and its old calibration.

## Targets and evidence

| Target | Floor for every craft dimension | Meaning |
| --- | --- | --- |
| `functional` | 6 | A functional/prototype presentation; never implies professional finish. |
| `professional` (default) | 8 | Deliberate, coherent craft suited to the product and audience. |
| `showcase` (`aaa` input alias) | 9 | Exceptional visible execution; not product-wide AAA certification. |

A requested score floor may increase these minima but cannot redefine professional downward.
The weakest dimension matters; an excellent average cannot conceal weak typography or mixed
assets. Anchors distinguish broken (0–4), prototype (5–6), competent/generic (7), professional
(8), exceptional (9) and rare reference quality (10). A score of 10 needs specific support.

Five dimensions ask for named visible evidence, coverage and a concrete improvement when below
the target: typography; composition/density; purpose/identity fit; asset/icon/style coherence;
and visible-state/detail consistency. Baseline layout, contrast and accessibility scores remain
diagnostics. A screenshot cannot establish keyboard accessibility or other unseen behaviour.

Model evidence is still a model claim. `quality_assessment` is provisional and its observations
need independent verification. `approval` is always `null`, for every model/provider, because
the changed rubric has not been calibrated. Missing craft fields or incomplete coverage cannot
be converted into a positive result. `legacy_scores_meet_threshold` is a diagnostic only.

`ui_test` and `ux_critic` continue to report their declared functional checks. Their approval
does not mean professional or AAA presentation. Report functional acceptance and craft separately.

## Reference bank

The [versioned anchors](../specs/visual-reference-anchors.md) are included in the model prompt.
They describe comparisons and failure cases; they are not training, a calibrated image dataset,
or proof that the model visited the linked sites. Primary sources include:

- [Linear's 2026 design refresh](https://linear.app/now/behind-the-latest-design-refresh): task
  hierarchy, predictable action placement and restrained interface chrome.
- [Stripe payments/reporting](https://stripe.com/payments/features): a comparison for dense data
  tools, rather than making every tracker into a sparse marketing page.
- [Steam Deck](https://www.steamdeck.com/en/software): a controller-oriented comparison.
- [Hades II](https://www.supergiantgames.com/games/hades-ii/): expressive asset/type/HUD cohesion.
- [Monument Valley](https://www.monumentvalleygame.com/): restrained composition and art direction.
- [Super Bomberman R 2](https://www.konami.com/games/bomberman/r2/us/en/): arcade/grid readability.
- [Apple layout guidance](https://developer.apple.com/design/human-interface-guidelines/layout)
  and [Xbox accessibility guidance](https://learn.microsoft.com/en-us/gaming/accessibility/guidelines):
  hierarchy and functional coverage, not an aesthetic stamp.

These are selected comparisons, not blanket endorsements of every screen or claims of measured
superiority. Apply the user's brief. A minimalist game need not imitate an ornate game, and a
tracker need not imitate a console. Popularity, budget, dark themes, glow, gradients, animation
or rounded cards do not by themselves earn a high score.

## Supplying actual visual comparisons

`visual_critic` accepts `quality_target`, an optional `quality_min`, and up to two local
`reference_images` paths. Replace illustrative paths with screenshots you have inspected and
have permission to use:

```json
{
  "image_path": "E:/AI/reviews/current-screen.png",
  "quality_target": "professional",
  "metadata": "Data-heavy tracker. Compare hierarchy and numeric density; preserve its identity.",
  "reference_images": ["E:/AI/reviews/approved-data-tool-reference.png"]
}
```

The subject is image 1; subsequent images are comparisons, never substitute subjects. Each
image is bounded to 5 MiB, with at most two references. Paths, hashes and image roles are
recorded so reviewers can establish what was actually supplied. No reference URL is silently
downloaded. Supplying comparisons to a configured cloud reviewer sends those images to that
provider, just like the subject image; omit them when that sharing is not intended.

Default target configuration:

```toml
[vision]
quality_target = "professional" # KRITIC_VISUAL_QUALITY_TARGET
visual_min = 6.0 # additional floor; professional still requires at least 8
```

This does not enable vision, install a model or change the deterministic default. Without a
vision backend, the calling agent can still inspect screenshots and apply the reference rubric.

## Regression and calibration limits

The user identified Towergle, Impossible Tower, Boom Boom Tower, Altos Tower, Cointraptions and
the tracker website as examples where historical passes did not meet their expectations, with
some later improved through personal direction. This feedback motivates the new criteria;
it is not a claim that every current version is poor, or a labelled screenshot dataset.

Calibration requires versioned captures of specific screens/viewports/states, the user's target
and human ratings, plus difficult competent-but-generic cases. Compare multiple models against
those ratings and hold out some examples before enabling any automatic quality gate. Current
offline tests establish parsing, evidence requirements, image binding and provisional verdict
semantics. They do not prove that a vision model now shares the user's taste.
