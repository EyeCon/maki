# Landing page design (`site/index.html`)

Kin to the docs, not a clone. See `docs/DESIGN.md` for the docs system; its banned list is docs-scoped and does not apply here.

Shared with the docs: the mono face (same `/docs/fonts/` URL, so the cache carries over), the sienna/salmon accent, the dark code panes and `maki` syntax theme, the theme toggle and its `localStorage.theme` key.

Owned by this page alone, do not flatten into docs vocabulary: a dusk sky behind the hero and the closing section with moon, stars and cloud silhouettes; warm paper and plum ink; 8-10px radii; no eyebrow labels.

## Type

Nunito (`--font-body`, self-hosted variable latin woff2, 39KB, preloaded) carries the wordmark, tagline, headings and body. Mono is the machine voice only: nav breadcrumb, install command, labels, code. Nothing is fetched from Google.

The wordmark is plain `maki` in Nunito 800 and nothing else. No caret, no accent block, no dot drawn on the `i`. Its metrics are load-bearing: `clamp(2.5rem, 5vw, 3.5rem)`, `-0.03em`, `line-height: 1.05`. Enlarging it to 4rem in a type pass made it read heavy and wrong even though the face was identical, so leave the size alone. Mono was tried (squat `m` at display size) and so was a display serif (Fraunces); both rejected.

## Theme

Dark is the default, in CSS, not script. `:root` holds the dark tokens and `[data-theme="light"]` overrides them, so a JS-less or pre-script paint is already dark and nothing flips. The inline head script only applies an explicit `light` choice. `prefers-color-scheme` is deliberately ignored, on this page and in the docs, so the two never disagree. Never set the background from script.

The canvas background lives on `html` and must equal the hero gradient's first stop (`--sky-1`); `.bottom-sky` returns to `--sky-1` at 100% so the bottom overscroll matches too. Fixed elements do not paint in the overscroll region, so there is only ever one colour to get right.

## Behaviour

Keyboard-first, like the app: `?` lists keys, `j`/`k` walk sections, `g`/`G` jump, `t` switches theme, `c` copies the install command, typing `doom` finds the DOOM demo. The moon in the hero is also the theme switch.

Nothing hijacks text selection, and there is no reading-progress bar on the nav. A line that fills left to right under a fixed header reads as a horizontal scrollbar; it was reported as one twice. The breadcrumb already says where you are.

Transitions are short: about 0.1s for hover and colour, 0.13-0.35s for entrances. Ambient sky motion is exempt (cloud drift 38s/52s, twinkle 6s, caret blink 1.15s); speeding it up reads as frantic, not fast.

## Install command

Not a widget. No border, no segmented copy button, no OS pill tabs, that trio being the most cloned component on the web. It is a shell line at hero scale on a soft fill with the prompt in accent; the whole panel is the copy button, and a small mono note sits outside it.

The platform link is phrased as a question (`on Windows?`), never a bare platform name, because a lone `Windows` under a macOS command reads as a status rather than an action. Nothing sits beside the command, so it wraps instead of cropping; the old flex row silently clipped its own copy button whenever the `nowrap` command outgrew the hero column.

## Demos

No borders. The dark body is the edge; an outline just boxes a box. They are lifted with `--demo-shadow` instead. Light mode needs two layers (a tight contact layer plus a wide ambient one) because one soft shadow vanishes against the pale sky; dark needs only one near-black layer.

Watch for clipping ancestors. The hero's shadow must sit on `.tui-scale-wrap`, not the `.tui` inside it, because the wrap is `overflow: hidden` to contain the scale transform. For the same reason `.doom-demo` must not carry `content-visibility: auto`, which implies paint containment and cropped the shadow flush to the frame.

Both demos are sized by content, not by the column. The hero cast was recorded at 86x22 and the player sizes itself from its own 18px font, so `scaleTui()` scales it to fit its grid column; it lands at 856px at desktop. That falls out of `.hero-grid` `max-width: 1280px` with `30% 1fr` columns and a 2.5rem gap, plus `.hero` padding `clamp(2rem, 4vw, 4rem)`. The `30%` matters below 1280px: a fixed `24rem` keeps the text column wide and starves the demo. The DOOM capture stops at 900px because the game renders into a 200x124 cell grid and only gets chunkier when stretched.

## Layout

One text axis. Every section heading and body block starts on the same left rule. Centring only the provider pills (two axes inside one section) and then centring the whole providers section (one section disagreeing with the other four) were both tried and rejected. The DOOM figure is the single centred element, because it is media that cannot fill its column, not a text block.

Provider chips are a wrapping flex row of content-width pills. An `auto-fit, minmax(190px, 1fr)` grid gives every chip the widest one's column, so `xAI` sits in a box of dead space. The ragged right edge of the last row is what a tag row looks like, not a defect. No logos: only 9 of the 17 providers have a mark in simple-icons, so the smaller ones, which are the point of the section, would look unfinished beside the big names.

Code panes drop to one column at 1220px, not 1000px, so the two-column squeeze never clips a line. Scrollbars are thinned globally via `--scroll-thumb`.

## Verifying

Serve `site/` (`python3 -m http.server`) and open `/index.html`. Check both themes at 390px and 1600px. Screenshot previews can lie about colour; pixel-sample PNGs.

Fonts live in `docs/static/fonts/` and are referenced as `/docs/fonts/...`, a path Zola only creates when it builds the docs. `site/docs/fonts` is a symlink to `static/fonts` so that URL also resolves when `site/` is served raw. Without it the page silently falls back to system fonts and the wordmark loses its rounded shapes, which looks like the wrong typeface rather than a missing file.
