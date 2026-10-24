package main

import (
	"flag"
	"fmt"
	"os"

	"dotfiles/tools/internal/print"
)

func main() {
	helpMessage := "usage: %s <Target> [-f|-n]\n"

	fileFlag := flag.Bool("f", false, "print file content")
	nameFlag := flag.Bool("n", false, "print filename")
	flag.Parse()

	if flag.NArg() != 1 {
		fmt.Fprintf(os.Stderr, helpMessage, os.Args[0])
		os.Exit(2)
	}
	target := flag.Arg(0)

	switch {
	case *fileFlag:
		if err := print.PrintFilePretty(target); err != nil {
			fmt.Fprintf(os.Stderr, "print-file-pretty: %v\n", err)
			os.Exit(1)
		}
	case *nameFlag:
		if err := print.PrintFileName(target); err != nil {
			fmt.Fprintf(os.Stderr, "print-file-pretty: %v\n", err)
			os.Exit(1)
		}
	default:
		fmt.Fprintf(os.Stderr, "print-file-pretty: unkown errror\n")
		os.Exit(2)

	}
}
