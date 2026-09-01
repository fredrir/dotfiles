# file-explorer

An inline, reusable file and location picker for command-line tools. Its state machine, rendering, data source, application-specific view, and terminal runtime are separate interfaces, so local filesystems and remote or virtual trees use the same interaction model.

```rust
use std::path::PathBuf;

use file_explorer::{AcceptTarget, EntryKind, Explorer, LocalSource, Outcome};
use workstation::Style;

fn choose_file(start: PathBuf) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let style = Style::plain();
    let outcome = Explorer::new(LocalSource::new(), start, &style)
        .accept_target(AcceptTarget::HighlightedEntry)
        .selectable(|kind| kind == EntryKind::File)
        .run()?;

    Ok(match outcome {
        Outcome::Selected(selection) => Some(selection.location),
        Outcome::Cancelled | Outcome::Interrupted | Outcome::Unavailable => None,
    })
}
```