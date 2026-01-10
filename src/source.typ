#import "@preview/touying:0.6.1": *
#import themes.metropolis: *
#import "@preview/physica:0.9.0": *

#show: metropolis-theme.with(
  aspect-ratio: "16-9",
  config-info(
    title: "Test presentation",
  ),
)
#set text(font: "IBM Plex Sans")

#title-slide()

== Simple text and image

#grid(
  columns: 2,
  lorem(20), image("_DSC0070.png"),
)

== Bullet points

- First item
- Second item
  - First sub-item

== Math text

#grid(
  columns: (1fr, 1fr),
  [
    This is a simple block equation :

    $ I = integral_0^infinity pi/x dd(x) $
  ],
  [Given the variable $x$, $y^3$, and $z_j$, can they be rendered correctly?],
)

== Shape

#align(center, polygon(
  fill: blue.lighten(80%),
  stroke: blue,
  (10%, 0pt),
  (30%, 0pt),
  (40%, 4cm),
  (0%, 4cm),
))

== Svg image

#image("monotonicity.svg")