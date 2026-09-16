package utils

import (
	"context"
	"fmt"

	"golang.design/x/clipboard"
)

func CopyString(content string) error {

	if err := clipboard.Init(); err != nil {
		return fmt.Errorf("init clipboard: %w", err)
	}
	if _, err := clipboard.Write(context.Background(), clipboard.FmtText, []byte(content)); err != nil {
		return fmt.Errorf("write to clipboard: %w", err)
	}

	return nil
}
