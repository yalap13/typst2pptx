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
#show link: underline
#show link: set text(blue.darken(20%))

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

== Shape <shape_slide>
#rotate(45deg,
align(center, polygon(
  fill: blue.lighten(80%),
  stroke: blue,
  (10%, 0pt),
  (30%, 0pt),
  (40%, 4cm),
  (0%, 4cm),
)))

== Svg image
#rotate(15deg,
image("monotonicity.svg")
)

== Shear/skew playground
#grid(
  columns: (1fr, 1fr),
  [
    #skew(ax: 20deg)[
      Skewed text block with horizontal shear.
    ]
    #skew(ax: -15deg, ay: 10deg,
      rect(
        width: 5cm,
        height: 2.5cm,
        fill: purple.lighten(70%),
        stroke: purple,
      )
    )
  ],
  [
    #skew(ax: 12deg,
      image("_DSC0070.png", width: 4cm)
    )
    #v(2em)
    #skew(ay: 18deg,
      [
        $integral_0^pi sin(x) d x$
        Skewed math + inline text
      ]
    )
  ],
)
== Transform playground
#rotate(scale($->$, 400%), -120deg)
#grid(
  columns: (1fr, 1fr),
  [
    #move(
      dx: 12pt,
      dy: -10pt,
      rotate(
        25deg,
        origin: center,
        reflow: true,
        scale(
          x: 120%,
          y: 90%,
          origin: center,
          reflow: true,
          box(
            width: 6cm,
            height: 2.4cm,
            fill: orange.lighten(80%),
            stroke: orange,
            inset: 12pt,
            align(center, [Rotated & scaled text]),
          ),
        ),
      ),
    )
    #move(
      dx: -8pt,
      dy: 16pt,
      scale(
        x: 90%,
        y: 110%,
        image("_DSC0070.png", width: 5cm),
      ),
    )
  ],
  [
    #rotate(
      -30deg,
      origin: center,
      reflow: true,
      polygon(
        fill: green.lighten(70%),
        stroke: green.darken(10%),
        (0pt, 0pt),
        (3cm, 0pt),
        (3.5cm, 2cm),
        (1.5cm, 2.8cm),
        (-0.5cm, 1.6cm),
      ),
    )
    #move(
      dx: 10pt,
      dy: -6pt,
      scale(
        x: 110%,
        y: 110%,
        reflow: true,
        [
          #rotate(90deg)[Math run: $integral_0^(2pi) sin(theta) d theta$]
          #rotate(-18deg)[Tilted label text]
        ],
      ),
    )
  ],
)

== Links

Test link to the #link("https://example.com")[See example.com]

And a link to a #link(<shape_slide>)[different slide]