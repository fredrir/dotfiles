# Address on the current machine that this SSH session arrived through.
_ssh_session_host() {
	[[ -n ${SSH_CONNECTION:-} ]] || return 1

	local -a conn
	conn=(${=SSH_CONNECTION})

	print -r -- "$conn[3]"
}

_ssh_session_transport() {
	local host
	host=$(_ssh_session_host) || return

	case "$host" in
	10.77.77.1 | 10.77.77.2)
		print USB
		;;
	*)
		# In your setup, a non-USB SSH connection is the Tailscale route.
		print Tailscale
		;;
	esac

}

# Find package manager by walking upward, so this also works
# from packages inside monorepos.
_dev_package_manager() {
	[[ -n ${DEV_PM:-} ]] && {
		print -r -- "$DEV_PM"
		return
	}

	local dir=$PWD

	while true; do
		[[ -f "$dir/pnpm-lock.yaml" ]] && {
			print pnpm
			return
		}

		[[ -f "$dir/bun.lock" || -f "$dir/bun.lockb" ]] && {
			print bun
			return
		}

		[[ -f "$dir/package-lock.json" ]] && {
			print npm
			return
		}

		[[ "$dir" == / ]] && break
		dir=${dir:h}
	done

	print -u2 -- "dev: couldn't determine package manager"
	return 1
}

dev() {
	local pm host transport flag

	pm=$(_dev_package_manager) || return

	flag=--hostname

	# Some dev servers, e.g. Vite, use --host.
	if [[ ${1:-} == --host || ${1:-} == --hostname ]]; then
		flag=$1
		shift
	fi

	if [[ -n ${SSH_CONNECTION:-} ]]; then
		host=$(_ssh_session_host) || return
		transport=$(_ssh_session_transport)

		case "$transport" in
		USB)
			print -P "%F{green}dev: $pm · USB · $host%f" >&2
			;;
		Tailscale)
			print -P "%F{blue}dev: $pm · Tailscale · $host%f" >&2
			;;
		*)
			print -P "%F{yellow}dev: $pm · SSH · $host%f" >&2
			;;
		esac
	fi

	case "$pm" in
	pnpm)
		if [[ -n ${host:-} ]]; then
			command pnpm run dev "$flag" "$host" "$@"
		else
			command pnpm run dev "$@"
		fi
		;;

	bun)
		if [[ -n ${host:-} ]]; then
			command bun run dev "$flag" "$host" "$@"
		else
			command bun run dev "$@"
		fi
		;;

	npm)
		if [[ -n ${host:-} ]]; then
			command npm run dev -- "$flag" "$host" "$@"
		else
			command npm run dev -- "$@"
		fi
		;;
	esac
}
