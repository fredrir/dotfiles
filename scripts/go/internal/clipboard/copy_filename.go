package clipboard

import (
	"path/filepath"

	"dotfiles/tools/internal/clipboard/utils"
)

func CopyFileName(path string) error {
	utils.CopyString(filepath.Base(path))
	return nil
}
