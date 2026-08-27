dotfile() {
	if [[ $1 == sync ]]; then
		local arg
		for arg in "$@"; do
			case $arg in
			-n | --dry-run | --help)
				command dotfile "$@"
				return
				;;
			esac
		done

		command dotfile "$@" || return
		exec zsh
	fi

	command dotfile "$@"
}
