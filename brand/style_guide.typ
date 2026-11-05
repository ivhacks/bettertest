// The bettertest brand guide. Build with ./build.sh in this directory, which makes the fonts first.

// Colors. Pass and fail belong to the product, not the brand.
#let black = rgb("#000000")
#let white = rgb("#ffffff")
#let red = rgb("#ff0000")
#let red-on-white = rgb("#d40000")
#let orange = rgb("#ff6e00")
#let amber = rgb("#ffb700")
#let yellow = rgb("#ffff00")
#let pass = rgb("#2ea44f")
#let fail = rgb("#e5484d")
#let ink = rgb("#111111")
#let card = rgb("#121212")
#let paper = rgb("#f3f3f3")
#let muted = rgb("#8a8a8a")
#let rule = rgb("#dddddd")
#let brand-gradient = gradient.linear(red, amber)

// Archivo moves along its width axis; each width is its own family (see fonts.py). `fill: auto` keeps the
// surrounding color.
#let paint(fill) = if fill == auto { (:) } else { (fill: fill) }
// A block with no spacing of its own: at hero sizes, paragraph spacing (in em) would open huge gaps.
#let hero(size, body, fill: auto) = block(spacing: 0pt, text(font: "Archivo 62", weight: 700, size: size, ..paint(fill), top-edge: "cap-height", bottom-edge: "baseline", upper(body)))
#let copy(body, size: 10pt, fill: auto) = text(font: "Archivo 80", size: size, ..paint(fill), body)
#let label(body, size: 7.5pt, fill: auto, width: 62) = text(font: "Archivo " + str(width), weight: 500, size: size, ..paint(fill), body)
#let mono(body, size: 8pt, fill: auto) = text(font: "Martian Mono 75", size: size, ..paint(fill), body)
#let eyebrow(body, fill: red) = text(font: "Archivo 80", weight: 600, size: 6.5pt, tracking: 0.14em, fill: fill, upper(body))

#set document(title: "bettertest brand guide", author: "bettertest")
#set page(paper: "us-letter", margin: (x: 0.62in, top: 0.62in, bottom: 0.75in))
#set text(font: "Archivo 80", size: 10pt, lang: "en")
#set par(leading: 0.62em)
// The guide's quotes are straight, like code.
#set smartquote(enabled: false)
#show raw: set text(font: "Martian Mono 75")

// A page: its ground, the eyebrow and title, the running footer.
#let section(number: none, eyebrow-text, title, intro: none, dark: false, ground: white, body) = {
  let fg = if dark { white } else { black }
  let accent = if dark { amber } else { red }
  set page(
    fill: if dark { black } else { ground },
    footer: context {
      set text(font: "Archivo 80", size: 6pt, tracking: 0.12em, fill: muted)
      let n = counter(page).get().first()
      [BETTERTEST BRAND GUIDE #h(1fr) #if n < 10 [0]#n]
    },
  )
  set text(fill: fg)
  eyebrow(eyebrow-text, fill: accent)
  v(16pt)
  hero(30pt, title)
  v(18pt)
  if intro != none {
    block(width: 80%, spacing: 0pt, copy(intro))
    v(18pt)
  }
  body
}

// The mark, drawn from the outline on the construction page, in mark units (y down).
#let outline-pts = ((24.356, 0), (114.356, 0), (50.034, 36.985), (105.109, 36.985), (0, 97.423))
#let play-pts = ((33.71, 10), (73.894, 10), (26.963, 36.985))
#let check-pts = ((23.173, 52.146), (31.6, 47.3), (28.502, 59.694), (50.605, 46.985), (64.648, 46.985), (17.717, 73.97))
#let at(p, s, dx: 0pt, dy: 0pt) = (dx + p.at(0) * s, dy + p.at(1) * s)
#let mark(height: 40pt, fill: auto) = {
  if fill == auto { image("logo_assets/bettertest-gradient.svg", height: height) } else {
    let s = height / 97.423
    box(width: 114.356 * s, height: height, {
      place(polygon(fill: fill, ..outline-pts.map(p => at(p, s))))
    })
  }
}

// Buttons and windows cut their top-left and bottom-right corners at 45 degrees.
#let cut-shape(w, h, c) = ((c, 0pt), (w, 0pt), (w, h - c), (w - c, h), (0pt, h), (0pt, c))
#let cut(body, fill: red, stroke: none, c: 6.5pt, inset: (x: 11pt, y: 8pt), fg: white) = context {
  let content = text(fill: fg, body)
  let m = measure(content)
  let w = m.width + 2 * inset.x
  let h = m.height + 2 * inset.y
  box(width: w, height: h, baseline: inset.y, {
    place(polygon(fill: fill, stroke: stroke, ..cut-shape(w, h, c)))
    place(dx: inset.x, dy: inset.y, content)
  })
}
#let button(body, kind: "primary") = {
  let b = text(font: "Archivo 70", weight: 500, size: 9pt, body)
  if kind == "primary" { cut(b) } else if kind == "outlined" {
    cut(b, fill: black, stroke: 0.6pt + rgb("#555555"))
  } else { cut(text(size: 8pt, b), fill: orange, fg: black, c: 4pt, inset: (x: 7pt, y: 5pt)) }
}

// Spec tables: a key column and a value column, thin rules between rows.
#let spec(..rows) = table(
  columns: (1.6in, 1fr),
  stroke: (x, y) => (top: if y == 0 { 0.8pt + black } else { 0.4pt + rule }, bottom: 0.4pt + rule),
  inset: (x: 0pt, y: 6pt),
  ..rows.pos().map(((k, v)) => (text(weight: 600, size: 8.5pt, k), text(size: 8.5pt, v))).flatten(),
)

// 1 · Cover

#page(fill: black, footer: none, margin: (x: 0.62in, top: 0.62in, bottom: 0.62in))[
  #set text(fill: white)
  #eyebrow([bettertest · brand guide · v1], fill: amber)
  #v(1.1in)
  #image("logo_assets/bettertest-gradient.svg", height: 2.1in)
  #v(0.4in)
  #hero(84pt)[bettertest]
  #v(0.4in)
  #block(width: 70%, copy(size: 12pt, fill: rgb("#dddddd"))[
    CI that doesn't suck. One mark, one gradient, one type family. Sharp edges, exact numbers, no decoration that
    doesn't earn its place.
  ])
  #v(1fr)
  #line(length: 100%, stroke: 0.4pt + rgb("#333333"))
  #v(0.3in)
  #grid(
    columns: (1fr,) * 5,
    ..(("01", "logo"), ("02", "construction"), ("03", "color"), ("04", "type"), ("05", "in use")).map(((n, name)) => [
      #text(font: "Archivo 80", size: 6.5pt, fill: amber, n) \
      #text(font: "Archivo 80", weight: 600, size: 7.5pt, tracking: 0.1em, upper(name))
    ]),
  )
]

// 2 · Logo

#let logo-tile(ground, label-text, logo, fg) = box(width: 100%, height: 1.95in, fill: ground, stroke: 0.4pt + rule, {
  place(center + horizon, dy: -6pt, logo)
  place(bottom + left, dx: 9pt, dy: -9pt, text(font: "Archivo 80", weight: 600, size: 5.5pt, tracking: 0.12em, fill: fg, upper(label-text)))
})

#section(
  [01 · logo],
  [The B],
  intro: [A forward-leaning B made of two blades. The top counter is a play button, the bottom counter is a check: run,
    then pass. There is no wordmark lockup yet. When the name is needed, set it in the hero style next to the mark.],
)[
  #grid(
    columns: (1fr, 1fr),
    logo-tile(black, [primary · gradient on black], mark(height: 1.05in), muted),
    logo-tile(white, [gradient on white · any size], mark(height: 1.05in), muted),
    logo-tile(black, [one color · white], image("logo_assets/bettertest-white.svg", height: 1.05in), muted),
    logo-tile(white, [one color · black], image("logo_assets/bettertest-black.svg", height: 1.05in), muted),
  )
  #v(18pt)
  #grid(
    columns: (2.85in, 1fr),
    gutter: 20pt,
    box(width: 100%, height: 2.6in, stroke: 0.4pt + rule, {
      // The mark in its clear-space box: x is a quarter of its height on every side.
      let s = 1.25in / 97.423
      let (w, h) = (114.356 * s, 97.423 * s)
      let x = h / 4
      let (ox, oy) = (0.42in, 0.42in)
      place(dx: ox - x, dy: oy - x, rect(width: w + 2 * x, height: h + 2 * x, fill: rgb("#fff0dd"), stroke: (paint: orange, thickness: 0.5pt, dash: "dashed")))
      place(dx: ox, dy: oy, rect(width: w, height: h, fill: white, stroke: 0.5pt + orange))
      place(dx: ox, dy: oy, mark(height: h, fill: black))
      place(dx: ox + w / 2, dy: oy - x, line(length: x, angle: 90deg, stroke: 0.6pt + red))
      place(dx: ox + w / 2 + 3pt, dy: oy - x + 2pt, text(size: 5.5pt, fill: red)[x])
      place(dx: ox - x, dy: oy + h / 2, line(length: x, stroke: 0.6pt + red))
      place(dx: ox - x + 1pt, dy: oy + h / 2 - 9pt, text(size: 5.5pt, fill: red)[x])
      place(dx: ox - x, dy: oy + h + x + 5pt, text(size: 5.5pt, fill: red)[x = ¼ of the mark's height])
    }),
    spec(
      ([Clear space], [x = ¼ of the mark's height on every side, measured from its bounding box.]),
      ([Next to type], [At least x from the box. Set the name in the hero style, cap height no taller than the mark.]),
      ([In a container], [The mark fills at most 60% of the container's shorter side.]),
      ([Minimum size], [24px tall on screen, 6mm in print. Favicon handling is still open.]),
      ([Never], [Stretch, rotate, re-slant, outline, add shadows or glows, add gradient stops, or put the gradient on a photo.]),
    ),
  )
]

// 3 · Construction

#let direction(sample, title, body) = box(width: 100%, height: 1.62in, fill: white, stroke: 0.4pt + rule, inset: 10pt, {
  align(center, box(height: 0.4in, align(horizon, sample)))
  v(4pt)
  hero(12pt, title)
  v(2pt)
  text(size: 7.5pt, body)
})

#section(
  [02 · construction],
  [Three directions, two strokes],
  ground: paper,
  intro: [Every edge is horizontal, slanted or diagonal. Every vertex is fixed by those directions and the stroke
    widths, so the mark can be redrawn exactly from this page. Units: y down, origin at the bottom point's x and the top
    edge.],
)[
  #grid(
    columns: (1fr, 1fr, 1fr),
    direction(line(length: 0.9in, stroke: 1.6pt), [H · 0°], [Top and middle bars, their inner edges, the top of the play, and the cut at the check's tip.]),
    direction(line(length: 0.36in, angle: 104.036deg, stroke: 1.6pt), [S · 14.036° from vertical], [Run 1 : rise 4. The left edge, the line through both tips, the play's left edge, and both edges of the check's short arm.]),
    direction(line(length: 0.7in, angle: -29.899deg, stroke: 1.6pt), [D · 29.899° from horizontal], [Rise 23 : run 40. Both outer diagonals, the play's hypotenuse, the check's long arm, and the top cut of its short arm.]),
  )
  #v(14pt)
  #box(width: 100%, height: 3.4in, fill: white, stroke: 0.4pt + rule, clip: true, {
    let s = 1.72pt
    let (ox, oy) = (2.05in, 0.72in)
    let p(pt) = at(pt, s, dx: ox, dy: oy)
    let guide = (paint: orange, thickness: 0.5pt, dash: "dashed")
    let small(body) = text(size: 5.5pt, fill: red, body)
    // Horizontal guides at the bars' edges.
    for y in (0, 10, 36.985, 46.985) {
      place(line(start: (ox - 0.9in, oy + y * s), end: (ox + 3.6in, oy + y * s), stroke: guide))
      place(dx: ox + 3.62in, dy: oy + y * s - 3pt, small[y #y])
    }
    // S lines: the left edge and the line through both tips; D lines through the tips.
    let s-line(pt) = line(start: p((pt.at(0) + 30 / 4, -30)), end: p((pt.at(0) - 115 / 4, 115)), stroke: 0.6pt + orange)
    place(s-line((24.356, 0)))
    place(s-line((114.356, 0)))
    place(dx: p((24.356 + 22 / 4, -22)).at(0) + 3pt, dy: p((0, -22)).at(1), small[S · left edge])
    place(dx: p((114.356 + 22 / 4, -22)).at(0) + 3pt, dy: p((0, -22)).at(1), small[S · tips])
    for (pt, name) in (((114.356, 0), "T1"), ((105.109, 36.985), "T2")) {
      place(line(start: p((pt.at(0) - 40, pt.at(1) + 23)), end: p((pt.at(0) + 40, pt.at(1) - 23)), stroke: 0.6pt + orange))
      place(dx: p(pt).at(0) + 0.32in, dy: p(pt).at(1) - 0.24in, small[D])
    }
    // The mark, outline and counters, with its named points.
    place(polygon(fill: rgb("#eeeeee"), stroke: 0.6pt + black, ..outline-pts.map(p)))
    place(polygon(fill: white, stroke: 0.6pt + black, ..play-pts.map(p)))
    place(polygon(fill: white, stroke: 0.6pt + black, ..check-pts.map(p)))
    for (pt, name, dx, dy) in (
      ((24.356, 0), "A", -10pt, -11pt), ((114.356, 0), "T1", 2pt, -11pt), ((50.034, 36.985), "N", -4pt, -12pt),
      ((105.109, 36.985), "T2", 2pt, -12pt), ((0, 97.423), "B", -10pt, 2pt),
    ) {
      place(dx: p(pt).at(0) - 1.6pt, dy: p(pt).at(1) - 1.6pt, circle(radius: 1.6pt, fill: black))
      place(dx: p(pt).at(0) + dx, dy: p(pt).at(1) + dy, text(weight: 700, size: 7pt, name))
    }
    // h, the bars' height.
    for y in (0, 36.985) {
      place(line(start: (ox - 0.75in, oy + y * s), end: (ox - 0.75in, oy + (y + 10) * s), stroke: 0.6pt + red))
      place(dx: ox - 0.88in, dy: oy + (y + 3) * s, small[h])
    }
    place(dx: ox - 0.45in, dy: 3.22in, small[t = 11.5 on every S and D stroke · h = 10 on every H stroke])
  })
  #v(14pt)
  #spec(
    ([Bounding box], [114.356 × 97.423]),
    ([Top bar], [90 long, from A to T1]),
    ([Stroke t], [11.5 on every slanted and diagonal stroke, measured perpendicular]),
    ([Stroke h], [10 on every horizontal bar, optically trimmed to 0.87t]),
    ([Tips], [T1 and T2 sit on one S line, parallel to the left edge]),
    ([Middle bar], [Top edge at y 36.985, touching the play's bottom vertex. Runs from N to T2]),
    ([Play counter], [Inset h from the top bar, t from the left edge and the diagonal: (33.71, 10), (73.894, 10), (26.963, 36.985)]),
    ([Outline], [A (24.356, 0) → T1 (114.356, 0) → N (50.034, 36.985) → T2 (105.109, 36.985) → B (0, 97.423). Even-odd fill]),
  )
]

// 4 · The check

#section(
  [02 · construction],
  [The check],
  ground: paper,
  intro: [Six points, all fixed by the B's own edges. The dashed triangle is the bottom counter, inset exactly like the
    play button.],
)[
  #box(width: 100%, height: 3.95in, fill: white, stroke: 0.4pt + rule, inset: 8pt, box(width: 100%, height: 100%, clip: true, {
    // x 12–70 and y 43–78 in mark units, the check and its labels, fill the box.
    let s = 7.4pt
    let (ox, oy) = (39pt - 12 * s, 6pt - 43 * s)
    let p(pt) = at(pt, s, dx: ox, dy: oy)
    // The B around the check, and the check itself, much enlarged.
    place(polygon(fill: rgb("#eeeeee"), ..outline-pts.map(p)))
    // The bottom counter the check sits in: S from P1 up to the bar's inner edge, then along it.
    let corner = (23.173 + (52.146 - 46.985) / 4, 46.985)
    place(curve(stroke: (paint: orange, thickness: 1.2pt, dash: "dashed"), curve.move(p((23.173, 52.146))), curve.line(p(corner)), curve.line(p((50.605, 46.985)))))
    place(polygon(fill: white, stroke: 1.2pt + black, ..check-pts.map(p)))
    for (i, pt) in check-pts.enumerate() {
      let (dx, dy) = ((-28pt, -14pt), (-6pt, -30pt), (4pt, 4pt), (-8pt, -30pt), (6pt, -22pt), (-28pt, 4pt)).at(i)
      place(dx: p(pt).at(0) - 3.2pt, dy: p(pt).at(1) - 3.2pt, circle(radius: 3.2pt, fill: black))
      place(dx: p(pt).at(0) + dx, dy: p(pt).at(1) + dy, hero(15pt)[P#(i + 1)])
    }
  }))
  #v(14pt)
  #spec(
    ([P6 → P1], [S line, inset t from the left edge. Collinear with the play's left edge]),
    ([P6 → P5], [D line, inset t from the lower diagonal]),
    ([P5], [Tip, where P6–P5 meets the middle bar's inner edge (y 46.985). P4–P5 lies on that edge]),
    ([Stroke c], [7 on both arms: P2–P3 is an S line c from P6–P1, P3–P4 a D line c from P6–P5]),
    ([P1 → P2], [D line, 27.7 from the lower diagonal's outer edge. This sets the short arm's length]),
    ([Proportion], [Short arm 22.5, long arm 54.1, about 1 : 2.4]),
    ([Points], [P1 (23.173, 52.146) P2 (31.6, 47.3) P3 (28.502, 59.694) P4 (50.605, 46.985) P5 (64.648, 46.985) P6 (17.717, 73.97)]),
  )
]

// 5 · Color

#let swatch(color, name, hex, body) = box(width: 100%, {
  block(width: 100%, height: 0.7in, fill: color, below: 0pt)
  block(width: 100%, height: 0.82in, fill: card, inset: 8pt, above: 0pt, {
    hero(13pt, fill: white, name)
    v(3pt)
    text(size: 7pt, fill: amber, hex)
    v(1pt)
    text(size: 7pt, fill: rgb("#bbbbbb"), body)
  })
})

#let contrast(sample-fg, sample-bg, name, rule-text, ratio) = (
  box(width: 1.15in, inset: 6pt, fill: sample-bg, align(center, text(weight: 600, size: 7.5pt, fill: sample-fg, name))),
  text(size: 7.5pt, rule-text),
  align(right, text(size: 7.5pt, ratio)),
)

#section(
  [03 · color],
  [Black ground, hot accents],
  dark: true,
  intro: [Mostly black and white. Red and orange do the shouting, amber only lives in the logo gradient, and yellow is a
    rare spark. Pass green and fail red are functional colors that belong to the product, not the brand.],
)[
  #grid(
    columns: (1fr, 1fr, 1fr),
    gutter: 1pt,
    swatch(black, [Black], [\#000000], [The primary ground: site, UI and decks.]),
    swatch(white, [White], [\#FFFFFF], [Type on black, and the secondary ground.]),
    swatch(red, [Red], [\#FF0000], [Primary accent: CTAs, key words, the gradient start.]),
    swatch(orange, [Orange], [\#FF6E00], [Secondary accent: eyebrows, highlights, syntax keywords.]),
    swatch(amber, [Amber], [\#FFB700], [The gradient end. Also code strings and small highlights on black.]),
    swatch(yellow, [Yellow], [\#FFFF00], [Rare spark: one element per view at most, on black only.]),
  )
  #v(14pt)
  #rect(width: 100%, height: 0.32in, fill: brand-gradient)
  #v(6pt)
  #text(size: 8pt)[The brand gradient: linear, two stops, \#FF0000 to \#FFB700, never with a middle stop. On the logo it
    runs in the mark's own coordinates from the top-right tip T1 (114.356, 0) to the bottom point B (0, 97.423). It's for
    the mark and rare hero moments, never for body text or UI fills.]
  #v(10pt)
  #table(
    columns: (1.25in, 1fr, auto),
    stroke: (x, y) => (top: if y == 0 { 0.6pt + white } else { none }, bottom: 0.4pt + rgb("#333333")),
    inset: (x: 0pt, y: 5pt),
    align: horizon,
    ..contrast(white, black, [White / Black], [Any size.], [21 : 1]),
    ..contrast(red, black, [Red / Black], [Any size.], [5.3 : 1]),
    ..contrast(orange, black, [Orange / Black], [Any size.], [7.3 : 1]),
    ..contrast(amber, black, [Amber / Black], [Any size.], [12 : 1]),
    ..contrast(red, white, [Red / White], [24px and up only. For smaller red text on white use \#D40000.], [4.0 : 1]),
    ..contrast(orange, white, [Orange / White], [Fills and graphics only, never text.], [2.9 : 1]),
  )
]

// 6 · Type

#let width-row(w, note, off: false) = (
  text(size: 7pt, fill: red)[w #w],
  if off { text(font: "Archivo 100", size: 11.5pt, tracking: 0.08em)[ArtifactsFromAnyRun.from_later_task · install_p…] } else {
    text(font: "Archivo " + str(w), size: 11.5pt)[ArtifactsFromAnyRun.from_later_task · install_packages]
  },
  text(size: 7pt, fill: rgb("#555555"), note),
)

#section(
  [04 · type],
  [Archivo for words, \ Martian Mono for code],
  intro: [One variable family carries everything from hero to grid labels by moving along its width axis. Archivo by
    Omnibus-Type and Martian Mono by Evil Martians are both SIL OFL 1.1: free to use, embed and ship.],
)[
  #block(width: 100%, fill: black, inset: (x: 16pt, y: 14pt), {
    set text(fill: white)
    eyebrow([hero], fill: amber)
    h(1fr)
    text(font: "Archivo 80", weight: 600, size: 6pt, tracking: 0.12em, fill: muted)[ARCHIVO · WIDTH 62 · WEIGHT 700]
    v(18pt)
    hero(40pt)[Go big or go die]
    v(16pt)
    text(size: 6.5pt, fill: rgb("#bbbbbb"))[All caps, line height 0.88–0.95, tracking 0. For headlines and section titles from 32px up.]
  })
  #v(10pt)
  #block(width: 100%, stroke: 0.4pt + rule, inset: (x: 16pt, y: 14pt), {
    eyebrow([copy])
    h(1fr)
    text(font: "Archivo 80", weight: 600, size: 6pt, tracking: 0.12em, fill: muted)[ARCHIVO · WIDTH 80 · WEIGHT 400]
    v(8pt)
    copy(size: 10.5pt)[Shared and pre-warmed containers shift as much processing as possible out of the hot path.
      Seconds are the difference between "we'll get back to you" and "it's deployed". Murphy's law says that if
      something can go wrong, it will. Cover everything so nothing can go wrong.]
    v(6pt)
    text(size: 6.5pt, fill: muted)[Sentence case, 15–18px on screen (10–12pt in print), line height 1.5–1.55, measure 60–75 characters. Bold is weight 600.]
  })
  #v(18pt)
  #hero(15pt)[The width axis]
  #v(8pt)
  #table(
    columns: (0.5in, 1fr, 1.4in),
    stroke: (x, y) => (top: if y == 0 { 0.8pt + black } else { none }, bottom: 0.4pt + rule),
    inset: (x: 0pt, y: 4.5pt),
    align: horizon,
    ..width-row(62, [Hero, grid labels, tight UI]),
    ..width-row(70, [Tags, table headers]),
    ..width-row(80, [Copy, the default]),
    ..width-row(90, [Long-form reading, optional]),
    ..width-row(100, [The ceiling]),
  )
  #v(4pt)
  #text(size: 7pt, fill: rgb("#555555"))[Copy sits at 80. Labels, grid headers and tags may go down to 62 when space is
    tight: narrow them, never shrink them below 11px. Above 100 is off-brand.]
]

// 7 · Type scale

#let scale-row(name, sample, spec-text) = (
  text(weight: 600, size: 7.5pt, name),
  sample,
  text(size: 6.5pt, fill: rgb("#555555"), spec-text),
)

#section([04 · type], [Type scale])[
  #table(
    columns: (0.95in, 1fr, 1.55in),
    stroke: (x, y) => (top: if y == 0 { 0.8pt + black } else { none }, bottom: 0.4pt + rule),
    inset: (x: 0pt, y: 9pt),
    align: horizon,
    ..scale-row([Display], hero(58pt)[Go big], [96 / 0.88 · w62 · 700 · caps]),
    ..scale-row([Headline], hero(34pt)[Keep your secrets], [56 / 0.95 · w62 · 700 · caps]),
    ..scale-row([Title], hero(19.5pt)[Don't get paged in the club], [32 / 1.0 · w62 · 700 · caps]),
    ..scale-row([Subhead], text(font: "Archivo 80", weight: 600, size: 13pt)[Designed to be airgapped], [22 / 1.3 · w80 · 600]),
    ..scale-row([Copy], copy(size: 10pt)[One binary, pipelines written in real Python, a web UI that shows green and red instantly.], [17 / 1.55 · w80 · 400]),
    ..scale-row([Small], copy(size: 8.5pt)[History lives in /var/tmp/bettertest and survives reboot.], [14 / 1.5 · w80 · 400]),
    ..scale-row([Eyebrow], eyebrow([05 · in use]), [12 · w80 · 600 · caps · +0.14em]),
    ..scale-row([UI label], label([install_packages · web_ui_visible · ArtifactsFromAnyRun]), [11–13 · w62–75 · 500]),
    ..scale-row([Code], mono(size: 8pt)[run("cd /src && cargo test")], [13 / 1.6 · Martian w75 · 400]),
  )
  #v(18pt)
  #hero(15pt)[Tokens]
  #v(8pt)
  #text(size: 7pt, fill: rgb("#555555"))[Load both families as variable fonts. Self-host for the airgapped UI; the OFL
    allows bundling them in the binary if the license file travels with them.]
  #v(6pt)
  #block(width: 100%, fill: black, inset: 12pt, mono(size: 7pt, fill: white, raw(block: true, lang: none,
    ":root {\n  --bt-black: #000000;  --bt-white: #ffffff;\n  --bt-red: #ff0000;    --bt-red-text-on-white: #d40000;\n  --bt-orange: #ff6e00; --bt-amber: #ffb700;  --bt-yellow: #ffff00;\n  --bt-gradient: linear-gradient(#ff0000, #ffb700);\n  --bt-font: \"Archivo\", sans-serif;\n  --bt-mono: \"Martian Mono\", monospace;\n}\n.bt-hero  { font: 700 96px/.88 var(--bt-font); font-stretch: 62%; text-transform: uppercase; }\n.bt-copy  { font: 400 17px/1.55 var(--bt-font); font-stretch: 80%; }\n.bt-label { font: 500 12px/1.2 var(--bt-font); font-stretch: 62%; }  /* 62-75 */\n.bt-code  { font: 400 13px/1.6 var(--bt-mono); font-stretch: 75%; }")))
]

// 8 · In use

#let code-block(body) = block(width: 100%, fill: card, stroke: 0.4pt + rgb("#2a2a2a"), inset: 14pt, {
  set text(font: "Martian Mono 75", size: 7.5pt, fill: white)
  set par(leading: 0.75em)
  body
})
#let kw(body) = text(fill: orange, body)
#let str-lit(body) = text(fill: amber, body)
#let comment(body) = text(fill: muted, body)

#let grid-labels = ("packages", "install_packages", "reboot_vm", "services_running", "web_ui_visible", "tests_run", "on_worker", "in_container", "in_vm", "intact", "latest", "pinned", "from_later_task", "wrong_name")
#let grid-runs = (("a46e0d", "pass"), ("acc7ea", "pass"), ("6b7845", "pass"), ("9a6012", "fail"), ("c250d2", "pass"), ("aad28b", "pass"))

#section(
  [05 · in use],
  [Put together],
  dark: true,
)[
  #block(width: 100%, fill: card, stroke: 0.4pt + rgb("#2a2a2a"), inset: (x: 18pt, y: 14pt), {
    eyebrow([leave your problems in the dust], fill: orange)
    v(18pt)
    hero(36pt)["It's deployed"]
    v(18pt)
    copy(size: 9.5pt, fill: rgb("#dddddd"))[Shared and pre-warmed containers shift as much processing as possible out
      of the hot path. \ Seconds are the difference between "we'll get back to you" and "it's deployed".]
    v(12pt)
    button([Get the binary])
    h(8pt)
    button([Read the docs], kind: "outlined")
  })
  #v(10pt)
  #code-block[
    #kw[from] bettertest #kw[import] Stage, run, task \
    \
    #kw[class] Test(Stage): \
    #h(2em)#comment[\# Each task gets its own container.] \
    #h(2em)\@task(IMAGE, labels=[Label.x86]) \
    #h(2em)#kw[def] unit(): \
    #h(4em)run(#str-lit["cd /src && cargo test"])
  ]
  #v(12pt)
  #text(font: "Archivo 80", weight: 600, size: 6pt, tracking: 0.12em, fill: muted)[RUN GRID · LABELS AT W62 · SQUARE CORNERS]
  #v(4pt)
  #block(width: 100%, fill: card, stroke: 0.4pt + rgb("#2a2a2a"), inset: (x: 10pt, top: 62pt, bottom: 10pt), {
    let cell = 0.32in
    grid(
      columns: (0.45in,) + (cell,) * grid-labels.len(),
      column-gutter: 1.5pt,
      row-gutter: 3pt,
      [],
      ..grid-labels.map(l => box(width: cell, height: 0pt, place(bottom + left, dx: 4pt, rotate(-62deg, origin: bottom + left, reflow: false, label(size: 7pt, fill: rgb("#cccccc"), l))))),
      ..grid-runs.map(((id, state)) => (
        mono(size: 6pt, fill: muted, id),
        ..range(grid-labels.len()).map(i => {
          let color = if state == "pass" { pass } else if i == 0 { fail } else { rgb("#1c1c1c") }
          rect(width: cell, height: 0.13in, fill: color)
        }),
      )).flatten(),
    )
  })
]

// 9 · Cut corners

#section(
  [05 · in use],
  [Cut corners],
  dark: true,
  intro: [Buttons, pop-up windows and other strong rectangles cut their top-left and bottom-right corners at exactly
    45°. The other two corners stay square. The cut is 10px on buttons 32–48px tall, 6px below that, and 16px on windows.
    Outlined shapes carry their 1px border around the cut. Windows whose content runs edge to edge, such as logs,
    terminals and the run grid, keep square corners so nothing gets clipped.],
)[
  #block(width: 100%, fill: card, stroke: 0.4pt + rgb("#2a2a2a"), inset: (x: 20pt, y: 20pt), {
    button([New run])
    h(10pt)
    button([Rerun from stage], kind: "outlined")
    h(10pt)
    button([forget], kind: "small")
    h(1fr)
    text(size: 6.5pt, fill: muted)[Primary · outlined · small (6px cut)]
  })
  #v(14pt)
  #context {
    let (width, height) = (3.35in, 1.3in)
    box(width: width, height: height, {
      place(polygon(fill: card, stroke: 0.6pt + rgb("#555555"), ..cut-shape(width, height, 10pt)))
      place(dx: 18pt, dy: 18pt, box(width: width - 36pt, {
        hero(16pt)[Forget worker?]
        v(12pt)
        copy(size: 8pt)[worker-03 stops getting tasks. A pipedef that names it will add it back.]
        v(10pt)
        align(right, {
          button([Cancel], kind: "outlined")
          h(8pt)
          button([Forget])
        })
      }))
    })
  }
  #v(14pt)
  #code-block(raw(block: true, lang: none,
    ".bt-cut {                /* top-left + bottom-right, 45° */\n  --cut: 10px;            /* 6px small, 16px windows */\n  clip-path: polygon(var(--cut) 0, 100% 0,\n    100% calc(100% - var(--cut)), calc(100% - var(--cut)) 100%,\n    0 100%, 0 var(--cut));\n}\n/* outlined: border color on the outer box, fill on the inner one,\n   inner --cut = outer - 0.586px, so the cut edge is exactly 1px thick */"))
]

// 10 · Button states

// What a button looks like in each state: its fill, its border (outlined) and its type.
#let state-button(body, fill, fg, stroke: none, small: false) = {
  let b = text(font: "Archivo 70", weight: 500, size: if small { 8pt } else { 9pt }, body)
  if small { cut(b, fill: fill, fg: fg, c: 4pt, inset: (x: 7pt, y: 5pt)) } else { cut(b, fill: fill, stroke: stroke, fg: fg) }
}
// color-mix(in srgb, fill, white 22%): how much a hovered button lights up.
#let lit(c) = color.mix((c, 78%), (white, 22%), space: rgb)
// CSS ease-out, cubic-bezier(0, 0, 0.58, 1), sampled at sixths of the fade.
#let fade-steps = ((0, 0), (70, 0.30), (140, 0.53), (210, 0.71), (280, 0.85), (350, 0.95), (420, 1))

#section(
  [05 · in use],
  [Hover, press, release],
  dark: true,
  intro: [A button answers the hand. Hovered, it lights up at once: its fill mixes in 22% white. Pressed, it turns
    white with black type the moment the mouse goes down, not when it comes up. Released, it fades back over 420 ms,
    easing out, to however it looks now: lit while the pointer is still on it.],
)[
  #block(width: 100%, fill: card, stroke: 0.4pt + rgb("#2a2a2a"), inset: (x: 20pt, y: 16pt), {
    let head(body) = text(font: "Archivo 80", weight: 600, size: 6pt, tracking: 0.12em, fill: muted, upper(body))
    grid(
      columns: (0.9in, 1fr, 1fr, 1fr),
      row-gutter: 14pt,
      align: horizon,
      [], head[Rest], head[Hover · at once], head[Pressed · mouse down],
      head[Primary],
      state-button([New Run], red, white),
      state-button([New Run], lit(red), white),
      state-button([New Run], white, black),
      head[Outlined],
      state-button([Rerun Stage], black, white, stroke: 0.6pt + rgb("#555555")),
      state-button([Rerun Stage], black, white, stroke: 0.6pt + lit(rgb("#555555"))),
      state-button([Rerun Stage], white, black, stroke: 0.6pt + white),
      head[Small],
      state-button([Forget], orange, black, small: true),
      state-button([Forget], lit(orange), black, small: true),
      state-button([Forget], white, black, small: true),
    )
  })
  #v(14pt)
  #text(font: "Archivo 80", weight: 600, size: 6pt, tracking: 0.12em, fill: muted)[RELEASED · 420 MS · EASE-OUT, BACK TO HOVER]
  #v(6pt)
  #grid(
    columns: (1fr,) * fade-steps.len(),
    gutter: 4pt,
    ..fade-steps.map(((ms, p)) => {
      let fill = color.mix((white, 100% - p * 100%), (lit(red), p * 100%), space: rgb)
      let fg = color.mix((black, 100% - p * 100%), (white, p * 100%), space: rgb)
      align(center, {
        state-button([New Run], fill, fg)
        v(5pt)
        mono(size: 6.5pt, fill: muted)[#ms ms]
      })
    }),
  )
  #v(16pt)
  #spec(
    ([Hover], [Fill mixes in 22% white, at once. Brightness cannot lift pure red, so it is a mix. Outlined buttons light their border.]),
    ([Pressed], [White fill, black type, on mouse down. Outlined buttons turn white through and through.]),
    ([Released], [Fades back over 420 ms, ease-out, wherever the pointer comes up, even after dragging off. Only a release fades: a hover never does.]),
  )
  #v(12pt)
  #code-block(raw(block: true, lang: none,
    "button:hover    { background: color-mix(in srgb, var(--fill), #fff 22%); }  /* at once */\nbutton:active   { background: #fff; color: #000; }                          /* mouse down */\nbutton.released { animation: released 420ms ease-out; }                     /* set on release */\n@keyframes released { from { background: #fff; color: #000; } }"))
]
