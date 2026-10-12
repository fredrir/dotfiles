package main

import (
	"runtime/debug"
	"strings"

	"github.com/jesseduffield/lazygit/pkg/app"
)

const module = "github.com/jesseduffield/lazygit"

// Built with patches/github.com/jesseduffield/lazygit.patch applied by dotfile sync.
func main() {
	app.Start(&app.BuildInfo{Version: version(), BuildSource: "dotfiles"}, nil)
}

func version() string {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return ""
	}
	for _, dependency := range info.Deps {
		if dependency.Path == module {
			return strings.TrimPrefix(dependency.Version, "v")
		}
	}
	return ""
}
