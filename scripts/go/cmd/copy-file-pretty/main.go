package main

import (
	"fmt"
	"os"

	"dotfiles/tools/internal/clipboard"
)

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintf(os.Stderr, "usage: %s FILE\n", os.Args[0])
		os.Exit(2)
	}
	if err := clipboard.CopyFilePretty(os.Args[1]); err != nil {
		fmt.Fprintf(os.Stderr, "copy-file-pretty: %v\n", err)
		os.Exit(1)
	}
}
