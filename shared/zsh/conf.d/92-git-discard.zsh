unalias gdd 2>/dev/null

# Throw away every change: tracked files back to HEAD, untracked files
# deleted. The plan is printed first, because half of it cannot be undone.
gdd() {
  emulate -L zsh

  local usage='usage: gdd [-n|--dry-run] [-a|--all] [-y|--yes] [path...]'
  local help="$usage

Discard every change in the working tree. Tracked files are restored to
HEAD and untracked files are deleted. Without a path the whole repository
is discarded; paths limit it to what they match.

Ignored files and nested repositories are kept.

Options:
  -n, --dry-run   Show what would be discarded and stop
  -a, --all       List every entry instead of the first 12 of a section
  -y, --yes       Discard without asking
  -h, --help      Show this help

The line counts are the diff against HEAD that would be thrown away. A
restored file is still in HEAD; a deleted untracked file is nowhere.

Examples:
  gdd                 Discard everything in the repository
  gdd -n              Show what that would be and stop
  gdd docs shared     Discard only what those paths match"

  local dry=0 all=0 yes=0
  while (( $# )); do
    case "$1" in
      -n|--dry-run) dry=1 ;;
      -a|--all)     all=1 ;;
      -y|--yes)     yes=1 ;;
      -h|--help)
        print -r -- "$help"
        return 0
        ;;
      --)
        shift
        break
        ;;
      -*)
        print -u2 -r -- "gdd: unknown option: $1"
        print -u2 -r -- "$usage"
        return 2
        ;;
      *)
        break
        ;;
    esac
    shift
  done

  local root
  root=$(command git rev-parse --show-toplevel 2>/dev/null)
  [[ -n $root ]] || {
    print -u2 -r -- 'gdd: not a git repository'
    return 1
  }

  # Before the first commit there is no tree to restore from. Against the
  # empty tree every staged file is an addition, which restore unstages and
  # removes: the same end state as restoring to a HEAD that holds nothing.
  local from=HEAD
  command git rev-parse --verify -q HEAD >/dev/null ||
    from=$(command git hash-object -t tree /dev/null)

  local entries
  entries=$(command git status --porcelain --no-renames -z --untracked-files=normal -- "$@") || return
  [[ -n $entries ]] || {
    print -r -- 'gdd: nothing to discard'
    return 0
  }

  local -A adds dels
  local numstat record rest
  numstat=$(command git diff --numstat --no-renames --no-relative -z $from -- "$@" 2>/dev/null)
  for record in ${(0)numstat}; do
    rest=${record#*$'\t'}
    adds[${rest#*$'\t'}]=${record%%$'\t'*}
    dels[${rest#*$'\t'}]=${rest%%$'\t'*}
  done

  local -i width=${COLUMNS:-0}
  (( width < 40 )) && width=100
  local -i cap=$(( width - 36 ))
  (( cap < 24 )) && cap=24

  local -a labels shown acol dcol notes
  local -a rows_restore rows_delete rows_kept
  local -a restore_specs clean_specs inside
  local -A tracked
  local code entry full label note add del text

  for record in ${(0)entries}; do
    code=${record[1,2]}
    entry=${record[4,-1]}
    full=$root/$entry
    label='' note='' add='' del=''

    if [[ $code == '??' ]]; then
      # A path can be staged as deleted and present on disk again; the
      # restore already covers it, so it is not also a deletion.
      [[ -n ${tracked[$entry]-} ]] && continue
      # git clean leaves a repository inside the tree alone, and so does the
      # plan, rather than promising a removal that will not happen.
      if [[ $entry == */ && -e ${full}.git ]]; then
        label=repository
        note='nested repository'
        rows_kept+=$(( $#labels + 1 ))
      else
        label=untracked
        if [[ $entry == */ ]]; then
          inside=( ${full}**/*(D^/N) )
          (( $#inside == 1 )) && note='1 file' || note="$#inside files"
        elif [[ -f $full ]]; then
          rest=$(command git -C $root diff --numstat --no-index -- /dev/null $entry 2>/dev/null)
          add=${rest%%$'\t'*}
        fi
        clean_specs+=$entry
        rows_delete+=$(( $#labels + 1 ))
      fi
    else
      add=${adds[$entry]-}
      del=${dels[$entry]-}
      case $code in
        (*U*|AA|DD) label=unmerged ;;
        (A*)        label=added ;;
        (D*|?D)     label=deleted ;;
        (T*|?T)     label=typechange ;;
        (*)         label=modified ;;
      esac
      tracked[$entry]=1
      restore_specs+=$entry
      if [[ $label == added ]]; then
        rows_delete+=$(( $#labels + 1 ))
      else
        rows_restore+=$(( $#labels + 1 ))
      fi
    fi

    [[ $add == '-' || $del == '-' ]] && note=binary
    labels+=$label
    notes+=$note
    text=${(V)entry}
    if (( ${#text} > cap )); then
      shown+="…${text[-cap+1,-1]}"
    else
      shown+=$text
    fi
    if [[ -n $add && $add != '-' && $add != 0 ]]; then acol+="+$add"; else acol+=''; fi
    if [[ -n $del && $del != '-' && $del != 0 ]]; then dcol+="-$del"; else dcol+=''; fi
  done

  local reset='' bold='' dim='' green='' red='' teal=''
  if [[ -t 1 && -z ${NO_COLOR-} ]]; then
    reset=$'\e[0m'
    bold=$'\e[1m'
    dim=$'\e[2m'
    green=${THEME_GIT:-$'\e[32m'}
    red=${THEME_SUDO:-$'\e[31m'}
    teal=${THEME_DIR:-$'\e[36m'}
  fi

  local -i limit=12
  (( all )) && limit=$#labels

  local -a visible=(
    ${rows_restore[1,limit]}
    ${rows_delete[1,limit]}
    ${rows_kept[1,limit]}
  )

  local -i i lw=0 pw=0 aw=0 dw=0 ta=0 td=0
  for i in $visible; do
    (( ${#labels[i]} > lw )) && lw=${#labels[i]}
    (( ${#shown[i]} > pw )) && pw=${#shown[i]}
    (( ${#acol[i]} > aw )) && aw=${#acol[i]}
    (( ${#dcol[i]} > dw )) && dw=${#dcol[i]}
  done
  for (( i = 1; i <= $#labels; i++ )); do
    (( ta += ${${acol[i]#+}:-0} ))
    (( td += ${${dcol[i]#-}:-0} ))
  done

  local section header tail line
  local -a rows
  print -r -- ''
  line="  ${bold}gdd${reset}  ${teal}${root/#$HOME/~}${reset}"
  (( $# )) && line+="  ${dim}$*${reset}"
  print -r -- "$line"

  for section in restore delete kept; do
    case $section in
      (restore)
        rows=( $rows_restore )
        header="restore to HEAD"
        ;;
      (delete)
        rows=( $rows_delete )
        header="${red}delete permanently${reset}"
        ;;
      (kept)
        rows=( $rows_kept )
        header="kept"
        ;;
    esac
    (( $#rows )) || continue

    print -r -- ''
    print -r -- "  ${bold}${header}${reset}"
    for i in ${rows[1,limit]}; do
      tail=''
      if [[ -n $notes[i] ]]; then
        tail="${green}${(l:$aw:)acol[i]}${reset} ${red}${(l:$dw:)dcol[i]}${reset}  ${dim}${notes[i]}${reset}"
      elif [[ -n $dcol[i] ]]; then
        tail="${green}${(l:$aw:)acol[i]}${reset} ${red}${(l:$dw:)dcol[i]}${reset}"
      elif [[ -n $acol[i] ]]; then
        tail="${green}${(l:$aw:)acol[i]}${reset}"
      fi
      if [[ -n $tail ]]; then
        print -r -- "    ${dim}${(r:$lw:)labels[i]}${reset}  ${(r:$pw:)shown[i]}  $tail"
      else
        print -r -- "    ${dim}${(r:$lw:)labels[i]}${reset}  $shown[i]"
      fi
    done
    (( $#rows > limit )) &&
      print -r -- "    ${dim}… and $(( $#rows - limit )) more${reset}"
  done

  local -a counts
  (( $#rows_restore )) && counts+="$#rows_restore restored"
  (( $#rows_delete )) && counts+="$#rows_delete deleted"
  (( $#rows_kept )) && counts+="$#rows_kept kept"
  line="  ${(j:, :)counts}"
  (( ta )) && line+="   ${green}+$ta${reset}"
  (( td )) && line+="  ${red}-$td${reset}"
  print -r -- ''
  print -r -- "$line"
  print -r -- ''

  (( dry )) && return 0
  (( $#restore_specs + $#clean_specs )) || {
    print -r -- 'gdd: nothing to discard'
    return 0
  }

  if (( ! yes )); then
    local reply
    while true; do
      if ! read -r "reply?Continue? [Y/n] "; then
        print
        return 1
      fi
      case "${reply:l}" in
        ''|y|yes)
          break
          ;;
        n|no)
          print -r -- 'gdd: cancelled'
          return 0
          ;;
        *)
          print -u2 -r -- 'Please answer y or n.'
          ;;
      esac
    done
  fi

  if (( $#restore_specs )); then
    print -rN -- $restore_specs |
      command git --literal-pathspecs -C $root restore --source=$from \
        --staged --worktree --pathspec-from-file=- --pathspec-file-nul || return
  fi

  if (( $#clean_specs )); then
    command git --literal-pathspecs -C $root clean -qfd -- $clean_specs || return
  fi

  print -r -- "  ${dim}done${reset}"
}
