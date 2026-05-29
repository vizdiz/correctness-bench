package migrations

import (
	"os"
	"regexp"
	"strings"
	"testing"
)

// TestMigrationMatchesFrozenContract guards against the goose migration drifting
// from contracts/schema.sql. Both are normalized (comments + whitespace stripped)
// and compared; if the frozen contract changes, this fails until the migration
// is updated to match.
func TestMigrationMatchesFrozenContract(t *testing.T) {
	contract, err := os.ReadFile("../../../contracts/schema.sql")
	if err != nil {
		t.Skipf("contract not readable from this path: %v", err)
	}
	migration, err := RawSchema()
	if err != nil {
		t.Fatalf("RawSchema: %v", err)
	}

	if normalizeSQL(string(contract)) != normalizeSQL(migration) {
		t.Fatalf("migration has drifted from contracts/schema.sql.\n--- contract ---\n%s\n--- migration ---\n%s",
			normalizeSQL(string(contract)), normalizeSQL(migration))
	}
}

var lineComment = regexp.MustCompile(`--[^\n]*`)
var wsRun = regexp.MustCompile(`\s+`)

func normalizeSQL(s string) string {
	s = lineComment.ReplaceAllString(s, "")
	s = wsRun.ReplaceAllString(s, " ")
	return strings.TrimSpace(s)
}
