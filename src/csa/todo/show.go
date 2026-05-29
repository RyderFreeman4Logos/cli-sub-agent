// File: src/csa/todo/show.go
// Package todo provides commands for managing TODO plans.
package todo

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// Constants for file structure and messages.
const (
	planDir         = ".todo"
	hashExt         = ".hash"
	planExt         = ".md"
	bannerMismatch  = "[PLAN TAMPERED] Plan content does not match stored attestation hash"
	bannerDraft     = "DRAFT – never attested"
	maxPlanFileSize = 1 << 20 // 1 MB, reasonable limit for plan content
)

// planIDPattern validates plan IDs (e.g., 20260529T051251).
var planIDPattern = regexp.MustCompile(`^\d{8}T\d{6}$`)

// AttestationState represents the possible states of plan attestation.
type AttestationState int

const (
	stateUnknown       AttestationState = iota
	stateNoHash                         // no stored hash
	stateHashMatch                      // hash matches
	stateHashMismatch                   // hash mismatch
)

// ShowOpts defines optional parameters for the Show function.
type ShowOpts struct {
	Logger *slog.Logger // structured logger; nil defaults to slog.Default()
	Stdout io.Writer    // output writer; nil defaults to os.Stdout
}

// Show displays a TODO plan with an attestation-aware header.
//
// It reads the plan file and its sidecar attestation hash, then displays
// one of three states:
//   - DRAFT – never attested (no .hash file)
//   - plan content (hash matches)
//   - security banner + plan content (hash mismatch)
//
// The plan ID must match the format YYYYMMDDTHHMMSS (e.g., 20260529T051251).
// Path traversal is prevented by rejecting IDs containing path separators
// and by ensuring the resolved plan directory is within the project base.
// The function returns an error if:
//   - planID is empty or has invalid format
//   - planID attempts path traversal
//   - plan file cannot be read (including not found)
//   - hash file cannot be read (excluding not found, which is treated as draft)
//   - writing to stdout fails
//
// Input validation: planID must match the expected pattern and must not contain
// directory separators or ".." sequences. The plan directory is resolved as
// a clean path relative to the current working directory.
func Show(planID string, opts ...ShowOpts) error {
	// Resolve options with defaults.
	var opt ShowOpts
	if len(opts) > 0 {
		opt = opts[0]
	}
	logger := opt.Logger
	if logger == nil {
		logger = slog.Default()
	}
	writer := opt.Stdout
	if writer == nil {
		writer = os.Stdout
	}

	// --- Input validation --------------------------------------------------
	if planID == "" {
		return fmt.Errorf("plan ID must not be empty")
	}
	if !planIDPattern.MatchString(planID) {
		return fmt.Errorf("invalid plan ID %q: must match YYYYMMDDTHHMMSS format", planID)
	}
	// Prevent path traversal.
	if strings.Contains(planID, "..") || strings.Contains(planID, string(os.PathSeparator)) {
		return fmt.Errorf("invalid plan ID %q: path traversal not allowed", planID)
	}

	// --- Build secure file paths -------------------------------------------
	// Use filepath.Clean to remove any accidental path separators from planDir.
	base := filepath.Clean(planDir)
	if !filepath.IsLocal(base) {
		return fmt.Errorf("plan directory %q is not local", planDir)
	}
	// Optionally resolve to absolute to prevent symlink escapes (optional hardening).
	absBase, err := filepath.Abs(base)
	if err != nil {
		return fmt.Errorf("failed to resolve plan directory absolute path: %w", err)
	}
	if !strings.HasPrefix(absBase, filepath.Clean(absBase)) {
		return fmt.Errorf("plan directory security check failed: path is not clean")
	}

	planPath := filepath.Join(absBase, planID+planExt)
	hashPath := filepath.Join(absBase, planID+hashExt)

	logger.Debug("resolved plan file path", "planID", planID, "planPath", planPath, "hashPath", hashPath)

	// --- Read plan content with size limit ---------------------------------
	// Use os.Open + io.LimitReader to prevent unbounded memory allocation.
	f, err := os.Open(planPath)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("plan %q not found at %s", planID, planPath)
		}
		return fmt.Errorf("failed to open plan %q: %w", planID, err)
	}
	defer f.Close()

	limitedReader := io.LimitReader(f, maxPlanFileSize)
	content, err := io.ReadAll(limitedReader)
	if err != nil {
		return fmt.Errorf("failed to read plan content %q: %w", planID, err)
	}
	if len(content) == maxPlanFileSize {
		// File is exactly the limit or larger; we might want to warn but still proceed.
		logger.Warn("plan file may exceed maximum allowed size; truncated", "planID", planID, "maxSize", maxPlanFileSize)
	}

	// --- Read stored attestation hash (if it exists) -----------------------
	storedHash, err := os.ReadFile(hashPath)
	if err != nil {
		if !errors.Is(err, os.ErrNotExist) {
			return fmt.Errorf("failed to read attestation hash for %q: %w", planID, err)
		}
		// Case 1: No stored hash – draft, never attested.
		logger.Info("plan displayed as draft (no attestation)", "planID", planID)
		if _, err := fmt.Fprintln(writer, bannerDraft); err != nil {
			return fmt.Errorf("failed to write draft banner to stdout: %w", err)
		}
		if _, err := writer.Write(content); err != nil {
			return fmt.Errorf("failed to write plan content to stdout: %w", err)
		}
		return nil
	}

	// --- Compute current hash ----------------------------------------------
	computedHash := sha256.Sum256(content)
	computedHex := hex.EncodeToString(computedHash[:])

	// Normalize stored hash: trim whitespace (newlines, spaces).
	storedHex := strings.TrimSpace(string(storedHash))

	// --- Compare and display -----------------------------------------------
	switch {
	case computedHex == storedHex:
		// Case 2: Hash matches – display normally.
		logger.Info("plan displayed with valid attestation", "planID", planID)
		if _, err := writer.Write(content); err != nil {
			return fmt.Errorf("failed to write plan content to stdout: %w", err)
		}
	default:
		// Case 3: Hash mismatch – display security banner.
		logger.Warn("plan attestation mismatch",
			"planID", planID,
			"computedHash", computedHex,
			"storedHash", storedHex,
		)
		if _, err := fmt.Fprintln(writer, bannerMismatch); err != nil {
			return fmt.Errorf("failed to write tamper banner to stdout: %w", err)
		}
		if _, err := writer.Write(content); err != nil {
			return fmt.Errorf("failed to write plan content to stdout: %w", err)
		}
	}

	return nil
}