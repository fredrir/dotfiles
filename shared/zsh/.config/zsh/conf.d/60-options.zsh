tardirs() {
  local archive="$1"
  local max_depth="${2:-}"
  local list_cmd

  if [[ -z "$archive" ]]; then
    echo "Usage: tardirs <archive.tar[.gz|.bz2|.xz]> [max-depth]" >&2
    return 1
  fi

  if [[ ! -f "$archive" ]]; then
    echo "File not found: $archive" >&2
    return 1
  fi

  case "$archive" in
    *.tar.gz|*.tgz)     list_cmd=(tar -tzf "$archive") ;;
    *.tar.bz2|*.tbz2)   list_cmd=(tar -tjf "$archive") ;;
    *.tar.xz|*.txz)     list_cmd=(tar -tJf "$archive") ;;
    *.tar)              list_cmd=(tar -tf "$archive") ;;
    *) echo "Unsupported archive: $archive" >&2; return 1 ;;
  esac

  "${list_cmd[@]}" |
    awk -v max_depth="$max_depth" '
      BEGIN {
        blue="\033[34m"
        cyan="\033[36m"
        green="\033[32m"
        yellow="\033[33m"
        bold="\033[1m"
        dim="\033[2m"
        reset="\033[0m"
      }

      {
        path=$0
        gsub(/^\.\//, "", path)

        # Treat files as contributing to their parent directory.
        if (path !~ /\/$/) {
          sub(/[^\/]+$/, "", path)
        }

        if (path == "") next

        sub(/\/$/, "", path)

        n=split(path, parts, "/")
        if (max_depth != "" && n > max_depth) next

        # Add every ancestor too, so the tree is connected.
        cur=""
        for (i=1; i<=n; i++) {
          cur = (cur == "" ? parts[i] : cur "/" parts[i])
          dirs[cur]=i
        }

        counts[path]++
      }

      END {
        for (d in dirs) {
          depth=dirs[d]

          parent=d
          if (parent ~ /\//) {
            sub(/\/[^\/]+$/, "", parent)
          } else {
            parent=""
          }

          base=d
          sub(/^.*\//, "", base)

          children[parent] = children[parent] d SUBSEP
          base_name[d]=base
          count[d]=(d in counts ? counts[d] : 0)
        }

        print bold cyan "Archive directory tree" reset
        print dim "count = direct archive entries mapped to that directory" reset
        print ""

        print_node("", "")
      }

      function print_node(parent, prefix,    raw, arr, n, i, child, connector, next_prefix) {
        raw=children[parent]
        if (raw == "") return

        n=split(raw, arr, SUBSEP)
        asort(arr)

        for (i=1; i<=n; i++) {
          child=arr[i]
          if (child == "") continue

          connector = (i == n-1 ? "└─ " : "├─ ")
          next_prefix = prefix (i == n-1 ? "   " : "│  ")

          printf "%s%s%s%s/%s", prefix, dim connector reset, bold, base_name[child], reset

          if (count[child] > 0) {
            printf "  %s[%s%s%s]%s", dim, green, count[child], dim, reset
          }

          printf "\n"

          print_node(child, next_prefix)
        }
      }
    '
}
