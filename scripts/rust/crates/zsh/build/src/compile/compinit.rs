//! oh-my-zsh's completion setup, with `compinit -C` while the dump is current.

use std::ops::{Range, RangeInclusive};

use zshrs_parse::parser::ZshList;

use super::walk;
use crate::script::{Script, Segment};

/// oh-my-zsh's dump check and `compinit` call, from `zcompdump_refresh=0`
/// through `unset -f _omz_compdump_has_metadata`.
const SECTION: u64 = 0x59ca_f595_845c_10b3;

pub struct Section {
    pub segments: Range<usize>,
    pub lists: Range<usize>,
    pub lines: RangeInclusive<usize>,
}

pub fn find(lists: &[ZshList], segments: &[Option<Segment>]) -> Option<Section> {
    let first = segments.iter().position(|segment| {
        segment
            .as_ref()
            .and_then(|segment| walk::sole_simple(&lists[segment.lists.start]))
            .is_some_and(|simple| {
                simple.words.is_empty()
                    && simple
                        .assigns
                        .first()
                        .is_some_and(|assign| assign.name == "zcompdump_refresh")
            })
    })?;
    let last = first
        + segments[first..].iter().position(|segment| {
            segment
                .as_ref()
                .and_then(|segment| walk::sole_simple(&lists[segment.lists.end - 1]))
                .is_some_and(|simple| {
                    walk::words(simple) == ["unset", "-f", "_omz_compdump_has_metadata"]
                })
        })?;
    let owned: Vec<&Segment> = segments[first..=last]
        .iter()
        .map(Option::as_ref)
        .collect::<Option<_>>()?;
    let lists_range = owned[0].lists.start..owned[owned.len() - 1].lists.end;
    if walk::fingerprint_lists(&lists[lists_range.clone()]) != SECTION {
        return None;
    }
    Some(Section {
        segments: first..last + 1,
        lists: lists_range,
        lines: *owned[0].lines.start()..=*owned[owned.len() - 1].lines.end(),
    })
}

/// The section as written, run only when the stamp next to the dump shows
/// it was built for a different `fpath`.
pub fn text(script: &Script, section: &Section) -> String {
    let original = script.lines(&section.lines);
    let newline = if original.ends_with('\n') { "" } else { "\n" };
    format!(
        "if [[ -r $ZSH_COMPDUMP && -r $ZSH_COMPDUMP.fpath && \"$(<$ZSH_COMPDUMP.fpath)\" == \"${{(F)fpath}}\" ]]; then\n\
         compinit -C -d \"$ZSH_COMPDUMP\"\n\
         unset zcompdump_revision zcompdump_fpath zcompdump_refresh\n\
         unset -f _omz_compdump_has_metadata\n\
         else\n\
         {original}{newline}\
         print -rl -- $fpath 2>/dev/null >| \"$ZSH_COMPDUMP.fpath\"\n\
         fi\n"
    )
}
