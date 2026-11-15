# Nabla brand assets

Drop the maintainer-provided logo files in this directory. They are served from
the web root, so the app references them as `/brand/<file>` and no code change is
needed once a file lands here.

## Expected files

| File | Use | Notes |
| --- | --- | --- |
| `nabla-logo.png` | Sidebar header (`<img src="/brand/nabla-logo.png">` in `src/components/AppSidebar.vue`) | Square metallic nabla. Rendered at 24px, so ship it at 128×128 or larger. |
| `nabla-logo-shadow.png` | Larger surfaces (splash, about, future empty states) | Same mark with a baked drop shadow on a transparent background. Not wired yet. |
| `nabla-logo-dark.png` | Dark palette variant, reserved | Not wired yet. Reserved for a theme-aware swap. |

## Behaviour while a file is missing

`nabla-logo.png` is optional at runtime: the `<img>` falls back to the ∇ glyph
(the del operator the app is named after) through an `@error` handler, so the app
looks correct before and after the asset is dropped in. No placeholder PNG is
generated on purpose — an absent file must stay absent so the fallback path is
the one exercised.

## Constraints

- PNG, transparent background, square canvas for `nabla-logo.png`.
- No `@2x` or `-small` variants: the mark is rendered at a fixed 24–28px.
- Keep the filenames above exact; they are referenced in code and docs.
