package copyutils

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"golang.design/x/clipboard"
)

func CopyFilePretty() {

	arguments := os.Args[1:]
	if len(arguments) == 0 {
		fmt.Println("Error: No target given")
		return
	} else if len(arguments) > 1 {
		fmt.Println("Error: Too many args")
		return
	}

	dir, err := os.Getwd()
	if err != nil {
		fmt.Println("Error read dir: ", err)
		return
	}

	givenTarget := filepath.Join(dir, arguments[0])
	relativePath, err := filepath.Rel(dir, givenTarget)
	if err != nil {
		fmt.Println("Error: ", err)
		return
	}

	fileExtension := strings.Trim(filepath.Ext(givenTarget), ".")
	fileBytes, err := os.ReadFile(givenTarget)

	if err != nil {
		fmt.Println("Error reading file: ", err)
		return
	}

	if err := clipboard.Init(); err != nil {
		fmt.Println("Clipboard error:", err)
		return
	}

	prettyContent := strings.TrimRight(fmt.Sprintf("---\n**%s:**\n```%s\n%s```\n---", relativePath, fileExtension, string(fileBytes)), "\r\n")

	clipboard.Write(context.Background(), clipboard.FmtText, []byte(prettyContent))

}
