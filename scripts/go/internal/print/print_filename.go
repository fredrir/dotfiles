package print

import (
	"fmt"
	"path/filepath"
)

func PrintFileName(path string) error {
	if _, err := fmt.Println(filepath.Base(path)); err != nil {
		return fmt.Errorf("print file: %w", err)
	}
	return nil
}
