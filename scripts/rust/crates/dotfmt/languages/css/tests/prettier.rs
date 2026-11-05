
use std::path::Path;

use dotfmt_css::{Config, Dialect, format};

fn formatted(input: &str) -> String {
    let config = Config {
        tailwind: dotfmt_tailwind::Config {
            mode: dotfmt_tailwind::Mode::Off,
            ..Default::default()
        },
        ..Config::default()
    };
    format(Path::new("fixture.css"), input, &config, Dialect::Css).expect("format")
}

#[test]
fn css_layout_matches_prettier() {
    let input = r#".card{display:grid;gap:1rem;padding:1.5rem}
.card:hover{transform:translateY(-2px)}
@media (min-width:40rem){.card{grid-template-columns:1fr 1fr}}
"#;
    let expected = r#".card {
  display: grid;
  gap: 1rem;
  padding: 1.5rem;
}
.card:hover {
  transform: translateY(-2px);
}
@media (min-width: 40rem) {
  .card {
    grid-template-columns: 1fr 1fr;
  }
}
"#;
    assert_eq!(formatted(input), expected);
}

#[test]
fn css_custom_properties_and_functions_match_prettier() {
    let input = r#":root{--brand:oklch(0.72 0.19 149.58);--space:calc(0.25rem * 4)}
.button{color:color-mix(in oklab,var(--brand),white 20%);background-image:linear-gradient(to right,var(--brand),transparent)}
"#;
    let expected = r#":root {
  --brand: oklch(0.72 0.19 149.58);
  --space: calc(0.25rem * 4);
}
.button {
  color: color-mix(in oklab, var(--brand), white 20%);
  background-image: linear-gradient(to right, var(--brand), transparent);
}
"#;
    assert_eq!(formatted(input), expected);
}
