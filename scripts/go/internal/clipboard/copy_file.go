package clipboard

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"golang.design/x/clipboard"
)

func CopyFilePretty(target string) error {
	dir, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("Error read dir: %w", err)
	}

	givenTarget := filepath.Join(dir, target)
	relativePath, err := filepath.Rel(dir, givenTarget)
	if err != nil {
		return fmt.Errorf("Error: %w", err)
	}

	fileExtension := strings.Trim(filepath.Ext(givenTarget), ".")
	fileBytes, err := os.ReadFile(givenTarget)
	if err != nil {
		return fmt.Errorf("Error reading file: %w", err)
	}

	if err := clipboard.Init(); err != nil {
		return fmt.Errorf("Clipboard error: %w", err)
	}

	prettyContent := strings.TrimRight(fmt.Sprintf("---\n**%s:**\n```%s\n%s```\n---", relativePath, fileExtension, string(fileBytes)), "\r\n")

	clipboard.Write(context.Background(), clipboard.FmtText, []byte(prettyContent))

	return nil
}
