# 0007 — Webview security baseline

- Status: Accepted
- Scope: Security
- Acceptance evidence: `D-010` (CSP active from day one, `style-src 'self' 'unsafe-inline'` accepted
  for Vuetify), recorded during `nabla-foundation` per ODD `odd/tasks/nabla-foundation.md` `L5`.
- Supersedes / superseded by: none

## Context

A Tauri webview renders local content but is still a browser: without a Content Security Policy it
will run remote script if a string ever reaches the wrong sink. The decision was taken at scaffold
time because a CSP added later is a rewrite of every inline style, font and asset assumption. The
reference project that motivated it shipped `"csp": null`, which disables the policy entirely.

## Decision

Keep CSP enabled in `src-tauri/tauri.conf.json` for every build, with this policy:

```text
default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; font-src 'self' data:
```

Tauri applies this policy to the packaged app's own assets. `npm run tauri dev` loads `devUrl`
(`http://localhost:1420`) from the Vite dev server, and the unused `security.devCsp` field means that
page is not covered by this policy: a policy violation must be confirmed against a packaged build.

`'unsafe-inline'` is accepted only in `style-src`, because the Tailwind v4 dev pipeline injects
styles at runtime (and some components set inline style attributes); it is not a licence to loosen `script-src`, `connect-src` or `default-src`. Revisit this entry when the app is packaged
(see Conditions to revisit). Bundled assets
must be satisfied by `'self'` or by an explicit entry added with a reason. `"csp": null` is a defect,
not an option.

## Alternatives

Disabling CSP to make a library work was rejected: it trades the whole policy for one dependency's
convenience. A hash/nonce-based policy is the strict alternative, but Vuetify's dynamic style
attributes cannot be hashed, so it would need a different styling approach; that is a future option,
not a current requirement. A stricter `default-src` with per-directive allowlists is what already
exists and must grow one reason at a time.

## Consequences

- Every remote resource, inline script or new scheme needs a deliberate policy entry with a stated
  reason; convenience edits to `tauri.conf.json` are review targets.
- Fonts and images must be local or `data:` URIs; CDN dependencies do not work and should not.
- Policy violations may only appear in a packaged build, so a change touching assets or libraries
  needs one `npm run tauri build` check, not just the dev window.

## Conditions to revisit

- If a dependency requires inline scripts, the successor record must name the dependency and the
  narrower directive it needs.
- When secrets or authenticated requests reach the webview (`T-004` is still open), review
  `connect-src` and the token path together, with that decision recorded here or in its successor.
