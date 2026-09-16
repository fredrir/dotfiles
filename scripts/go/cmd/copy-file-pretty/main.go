package main

import (
	"flag"
	"fmt"
	"os"

	"dotfiles/tools/internal/clipboard"
)

func main() {
	helpMessage := "usage: %s <Target> [-f|-n]\n"

	fileFlag := flag.Bool("f", false, "copy file content")
	nameFlag := flag.Bool("n", false, "copy filename")
	flag.Parse()

	if flag.NArg() != 1 {
		fmt.Fprintf(os.Stderr, helpMessage, os.Args[0])
		os.Exit(2)
	}
	target := flag.Arg(0)

	switch {
	case *fileFlag:
		if err := clipboard.CopyFilePretty(target); err != nil {
			fmt.Fprintf(os.Stderr, "copy-file-pretty: %v\n", err)
			os.Exit(1)
		}
	case *nameFlag:
		if err := clipboard.CopyFileName(target); err != nil {
			fmt.Fprintf(os.Stderr, "copy-file-pretty: %v\n", err)
			os.Exit(1)
		}
	default:
		fmt.Fprintf(os.Stderr, "copy-file-pretty: unkown errror\n")
		os.Exit(2)

	}
}
