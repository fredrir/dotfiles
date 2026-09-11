# ui-file-explorer

| API | Behavior |
| --- | --- |
| `FileSource` | Native, remote or virtual directory identities |
| `ExplorerView` | Application headers, badges and acceptance labels |
| `Explorer::run` | Borrowed synchronous source |
| `Explorer::into_async().run` | Owned source, bounded worker, immediate UI cancellation |
| `Cancellation` | Cooperative source cancellation; blocked sources may outlive the UI |
| `Terminal` | Scripted input, resize events and captured frames |
| `Outcome` | Single selection, cancellation, interruption or unavailable terminal |

```rust
use std::path::PathBuf;

use ui_file_explorer::{AcceptTarget, EntryKind, Explorer, LocalSource, Outcome};
use ui_theme::Style;

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

```sh
dotfile dev check -p ui-file-explorer,hcopy,dcloud
```
