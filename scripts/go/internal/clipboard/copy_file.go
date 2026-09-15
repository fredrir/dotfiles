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
	absPath, err := filepath.Abs(target)
	if err != nil {
		return fmt.Errorf("resolve path: %w", err)
	}

	fileBytes, err := os.ReadFile(absPath)
	if err != nil {
		return fmt.Errorf("read file: %w", err)
	}

	if err := clipboard.Init(); err != nil {
		return fmt.Errorf("init clipboard: %w", err)
	}

	ext := strings.TrimPrefix(filepath.Ext(absPath), ".")
	content := fmt.Sprintf("---\n**%s:**\n```%s\n%s```\n---\n", absPath, ext, string(fileBytes))

	if _, err := clipboard.Write(context.Background(), clipboard.FmtText, []byte(content)); err != nil {
		return fmt.Errorf("write to clipboard: %w", err)
	}

	return nil
}
