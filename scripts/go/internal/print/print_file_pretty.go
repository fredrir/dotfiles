package print

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func PrintFilePretty(target string) error {
	absPath, err := filepath.Abs(target)
	if err != nil {
		return fmt.Errorf("resolve path: %w", err)
	}

	fileBytes, err := os.ReadFile(absPath)
	if err != nil {
		return fmt.Errorf("read file: %w", err)
	}

	ext := strings.TrimPrefix(filepath.Ext(absPath), ".")
	content := fmt.Sprintf("---\n**%s:**\n```%s\n%s```\n---\n", absPath, ext, string(fileBytes))

	_, err = fmt.Fprintln(os.Stdout, content)

	if err != nil {
		return fmt.Errorf("print content: %w", err)
	}

	return nil
}
