package main

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

// Level represents log severity.
type Level int

const (
	LevelDebug Level = iota
	LevelInfo
	LevelWarn
	LevelError
)

func (l Level) String() string {
	switch l {
	case LevelDebug:
		return "DEBUG"
	case LevelInfo:
		return "INFO"
	case LevelWarn:
		return "WARN"
	case LevelError:
		return "ERROR"
	default:
		return "UNKNOWN"
	}
}

// Logger provides leveled, thread‑safe logging with optional structured fields.
type Logger struct {
	mu     sync.Mutex
	inner  *log.Logger
	level  Level
	fields map[string]string
	prefix string
}

// NewLogger creates a new Logger writing to w with the given minimum level.
func NewLogger(w io.Writer, level Level) *Logger {
	return &Logger{
		inner: log.New(w, "", log.LstdFlags|log.Lshortfile),
		level: level,
	}
}

// SetLevel changes the minimum log level.
func (l *Logger) SetLevel(level Level) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.level = level
}

// WithFields returns a new Logger with additional structured fields.
func (l *Logger) WithFields(fields map[string]string) *Logger {
	l.mu.Lock()
	defer l.mu.Unlock()
	merged := make(map[string]string, len(l.fields)+len(fields))
	for k, v := range l.fields {
		merged[k] = v
	}
	for k, v := range fields {
		merged[k] = v
	}
	return &Logger{
		inner:  l.inner,
		level:  l.level,
		fields: merged,
		prefix: l.prefix,
	}
}

// WithPrefix returns a new Logger with a prefix prepended to each message.
func (l *Logger) WithPrefix(prefix string) *Logger {
	l.mu.Lock()
	defer l.mu.Unlock()
	return &Logger{
		inner:  l.inner,
		level:  l.level,
		fields: l.fields,
		prefix: l.prefix + prefix,
	}
}

func (l *Logger) log(level Level, format string, v ...interface{}) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if level < l.level {
		return
	}
	var b strings.Builder
	b.WriteString("[")
	b.WriteString(level.String())
	b.WriteString("] ")
	if l.prefix != "" {
		b.WriteString(l.prefix)
	}
	if len(l.fields) > 0 {
		first := true
		for k, val := range l.fields {
			if !first {
				b.WriteByte(' ')
			}
			b.WriteString(k)
			b.WriteByte('=')
			b.WriteString(val)
			first = false
		}
		b.WriteByte(' ')
	}
	b.WriteString(fmt.Sprintf(format, v...))
	l.inner.Output(3, b.String())
}

// Debug logs at Debug level.
func (l *Logger) Debug(format string, v ...interface{}) { l.log(LevelDebug, format, v...) }

// Info logs at Info level.
func (l *Logger) Info(format string, v ...interface{}) { l.log(LevelInfo, format, v...) }

// Warn logs at Warn level.
func (l *Logger) Warn(format string, v ...interface{}) { l.log(LevelWarn, format, v...) }

// Error logs at Error level.
func (l *Logger) Error(format string, v ...interface{}) { l.log(LevelError, format, v...) }

// Fatal logs at Error level and exits with status 1.
func (l *Logger) Fatal(format string, v ...interface{}) {
	l.log(LevelError, format, v...)
	os.Exit(1)
}

var logger = NewLogger(os.Stderr, LevelInfo)

// ---------------------------------------------------------------------------
// File system abstraction
// ---------------------------------------------------------------------------

// FileSystem provides an abstraction over the OS filesystem for testability.
type FileSystem interface {
	Stat(name string) (fs.FileInfo, error)
	ReadFile(name string) ([]byte, error)
	WriteFile(name string, data []byte, perm fs.FileMode) error
	Open(name string) (io.ReadCloser, error)
	CreateTemp(dir, pattern string) (*os.File, error)
	MkdirTemp(dir, pattern string) (string, error)
	Rename(oldpath, newpath string) error
	Remove(name string) error
	RemoveAll(name string) error
}

// defaultFS implements FileSystem using the os package.
type defaultFS struct{}

func (defaultFS) Stat(name string) (fs.FileInfo, error)                { return os.Stat(name) }
func (defaultFS) ReadFile(name string) ([]byte, error)                 { return os.ReadFile(name) }
func (defaultFS) WriteFile(name string, data []byte, perm fs.FileMode) error {
	return os.WriteFile(name, data, perm)
}
func (defaultFS) Open(name string) (io.ReadCloser, error)             { return os.Open(name) }
func (defaultFS) CreateTemp(dir, pattern string) (*os.File, error)    { return os.CreateTemp(dir, pattern) }
func (defaultFS) MkdirTemp(dir, pattern string) (string, error)       { return os.MkdirTemp(dir, pattern) }
func (defaultFS) Rename(oldpath, newpath string) error                 { return os.Rename(oldpath, newpath) }
func (defaultFS) Remove(name string) error                             { return os.Remove(name) }
func (defaultFS) RemoveAll(name string) error                          { return os.RemoveAll(name) }

// ---------------------------------------------------------------------------
// Custom error types
// ---------------------------------------------------------------------------

var (
	ErrMissingTimestamp   = errors.New("timestamp must not be empty")
	ErrInvalidHashFile    = errors.New("invalid attestation hash file")
	ErrPlanDirNotSet      = errors.New("plan directory not set")
	ErrInvalidPlanContent = errors.New("plan content is empty")
)

// ErrInvalidTimestamp indicates the timestamp string does not match expected format.
type ErrInvalidTimestamp struct {
	Timestamp string
}

func (e *ErrInvalidTimestamp) Error() string {
	return fmt.Sprintf("invalid timestamp %q: expected format YYYYMMDDTHHMMSS", e.Timestamp)
}

// ErrPlanDirAccess indicates the plan directory cannot be accessed.
type ErrPlanDirAccess struct {
	Path string
	Err  error
}

func (e *ErrPlanDirAccess) Error() string {
	return fmt.Sprintf("cannot access plan directory %q: %v", e.Path, e.Err)
}
func (e *ErrPlanDirAccess) Unwrap() error { return e.Err }

// ErrPlanDirNotDirectory indicates the plan path exists but is not a directory.
type ErrPlanDirNotDirectory struct {
	Path string
}

func (e *ErrPlanDirNotDirectory) Error() string {
	return fmt.Sprintf("plan path %q exists but is not a directory", e.Path)
}

// ErrPlanNotFound indicates the plan file was not found.
type ErrPlanNotFound struct {
	Path string
}

func (e *ErrPlanNotFound) Error() string {
	return fmt.Sprintf("plan file not found: %s", e.Path)
}

// ErrHashMismatch indicates the computed hash does not match stored hash.
type ErrHashMismatch struct {
	PlanTimestamp string
	Expected      string
	Actual        string
}

func (e *ErrHashMismatch) Error() string {
	return fmt.Sprintf("plan %s: hash mismatch (expected %s, computed %s)",
		e.PlanTimestamp, e.Expected, e.Actual)
}

// ErrAttestationMissing indicates no attestation hash file exists for the plan.
type ErrAttestationMissing struct {
	PlanTimestamp string
	HashPath      string
}

func (e *ErrAttestationMissing) Error() string {
	return fmt.Sprintf("plan %s: attestation hash file missing: %s", e.PlanTimestamp, e.HashPath)
}

// ---------------------------------------------------------------------------
// Timestamp validation
// ---------------------------------------------------------------------------

// isValidTimestamp checks if the string matches the format YYYYMMDDTHHMMSS and is a valid date.
func isValidTimestamp(s string) bool {
	if len(s) != 15 {
		return false
	}
	for i, c := range s {
		if i == 8 {
			if c != 'T' {
				return false
			}
			continue
		}
		if c < '0' || c > '9' {
			return false
		}
	}
	_, err := time.Parse("20060102T150405", s)
	return err == nil
}

// ---------------------------------------------------------------------------
// Attestation configuration and service
// ---------------------------------------------------------------------------

const (
	planFileSuffix   = ".md"
	attestFileSuffix = ".hash"
	defaultPlanDir   = "./plans"

	filePerm      fs.FileMode = 0644
	dirPerm       fs.FileMode = 0755
	tmpFilePrefix             = ".attest_tmp_"
)

// AttestOptions holds all configuration for an attestation operation.
type AttestOptions struct {
	Timestamp string
	PlanDir   string
	FS        FileSystem
}

// Validate checks that all required fields are present and valid.
func (opts *AttestOptions) Validate() error {
	if opts.Timestamp == "" {
		return ErrMissingTimestamp
	}
	if !isValidTimestamp(opts.Timestamp) {
		return &ErrInvalidTimestamp{Timestamp: opts.Timestamp}
	}
	if opts.PlanDir == "" {
		opts.PlanDir = defaultPlanDir
	}
	if opts.FS == nil {
		opts.FS = defaultFS{}
	}
	return nil
}

// PlanFilePath returns the full path to the plan file based on options.
func (opts *AttestOptions) PlanFilePath() string {
	return filepath.Join(opts.PlanDir, opts.Timestamp+planFileSuffix)
}

// HashFilePath returns the full path to the attestation hash file.
func (opts *AttestOptions) HashFilePath() string {
	return filepath.Join(opts.PlanDir, opts.Timestamp+attestFileSuffix)
}

// AttestationService provides methods for computing, storing, and verifying plan attestations.
type AttestationService struct {
	fs FileSystem
	// For future concurrency support; currently single‑threaded.
	mu sync.Mutex
}

// NewAttestationService creates a new service with the given filesystem.
func NewAttestationService(fs FileSystem) *AttestationService {
	if fs == nil {
		fs = defaultFS{}
	}
	return &AttestationService{fs: fs}
}

// ComputeHash reads the plan file and returns its SHA256 hex digest.
func (s *AttestationService) ComputeHash(filePath string) (string, error) {
	logger.Debug("Computing hash for %s", filePath)
	data, err := s.fs.ReadFile(filePath)
	if err != nil {
		if os.IsNotExist(err) {
			return "", &ErrPlanNotFound{Path: filePath}
		}
		return "", fmt.Errorf("reading plan file %q: %w", filePath, err)
	}
	if len(data) == 0 {
		return "", ErrInvalidPlanContent
	}
	h := sha256.Sum256(data)
	hash := hex.EncodeToString(h[:])
	logger.Debug("Computed hash for %s: %s", filePath, hash)
	return hash, nil
}

// ReadHashFile reads the attestation hash from the hash file. Returns ErrAttestationMissing if file does not exist.
func (s *AttestationService) ReadHashFile(hashPath string) (string, error) {
	logger.Debug("Reading hash file %s", hashPath)
	data, err := s.fs.ReadFile(hashPath)
	if err != nil {
		if os.IsNotExist(err) {
			return "", &ErrAttestationMissing{HashPath: hashPath}
		}
		return "", fmt.Errorf("reading hash file %q: %w", hashPath, err)
	}
	hash := strings.TrimSpace(string(data))
	if len(hash) != 64 {
		return "", fmt.Errorf("%w: %s contains invalid hash length", ErrInvalidHashFile, hashPath)
	}
	logger.Debug("Read hash from %s: %s", hashPath, hash)
	return hash, nil
}

// WriteHashFile writes the hash atomically: writes to a temp file then renames.
func (s *AttestationService) WriteHashFile(hashPath, hash string) error {
	logger.Debug("Writing hash %s to %s", hash, hashPath)
	dir := filepath.Dir(hashPath)
	tmpFile, err := s.fs.CreateTemp(dir, tmpFilePrefix)
	if err != nil {
		return fmt.Errorf("creating temporary file in %q: %w", dir, err)
	}
	tmpPath := tmpFile.Name()
	// Clean up temp file in case of error
	defer func() {
		if tmpFile != nil {
			_ = s.fs.Remove(tmpPath)
		}
	}()

	if _, err := tmpFile.WriteString(hash + "\n"); err != nil {
		_ = tmpFile.Close()
		return fmt.Errorf("writing hash to temp file %q: %w", tmpPath, err)
	}
	if err := tmpFile.Close(); err != nil {
		return fmt.Errorf("closing temp file %q: %w", tmpPath, err)
	}
	if err := s.fs.Rename(tmpPath, hashPath); err != nil {
		return fmt.Errorf("renaming temp file %q to %q: %w", tmpPath, hashPath, err)
	}
	tmpFile = nil // prevent cleanup on rename success
	logger.Info("Hash file written: %s", hashPath)
	return nil
}

// Attest computes the SHA256 hash of the plan file and writes it to the corresponding .hash file.
func (s *AttestationService) Attest(opts AttestOptions) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	if err := opts.Validate(); err != nil {
		return fmt.Errorf("attest options validation: %w", err)
	}

	planPath := opts.PlanFilePath()
	hashPath := opts.HashFilePath()

	// Ensure plan directory exists
	if _, err := s.fs.Stat(opts.PlanDir); err != nil {
		if os.IsNotExist(err) {
			return &ErrPlanDirAccess{Path: opts.PlanDir, Err: err}
		}
		return &ErrPlanDirAccess{Path: opts.PlanDir, Err: err}
	}

	// Check plan file exists
	if _, err := s.fs.Stat(planPath); err != nil {
		if os.IsNotExist(err) {
			return &ErrPlanNotFound{Path: planPath}
		}
		return fmt.Errorf("checking plan file %q: %w", planPath, err)
	}

	hash, err := s.ComputeHash(planPath)
	if err != nil {
		return fmt.Errorf("computing hash: %w", err)
	}

	if err := s.WriteHashFile(hashPath, hash); err != nil {
		return fmt.Errorf("writing hash file: %w", err)
	}

	logger.Info("Attestation successful for plan %s", opts.Timestamp)
	return nil
}

// Verify checks that the current plan file hash matches the stored attestation hash.
// Returns nil if they match, or an error (ErrHashMismatch, ErrAttestationMissing, etc.) otherwise.
func (s *AttestationService) Verify(opts AttestOptions) error {
	if err := opts.Validate(); err != nil {
		return fmt.Errorf("verify options validation: %w", err)
	}

	planPath := opts.PlanFilePath()
	hashPath := opts.HashFilePath()

	// Compute hash of current plan
	currentHash, err := s.ComputeHash(planPath)
	if err != nil {
		return fmt.Errorf("computing current hash: %w", err)
	}

	// Read stored hash
	storedHash, err := s.ReadHashFile(hashPath)
	if err != nil {
		// If hash file does not exist, return specific error
		var missingErr *ErrAttestationMissing
		if errors.As(err, &missingErr) {
			return missingErr
		}
		return fmt.Errorf("reading stored hash: %w", err)
	}

	if currentHash != storedHash {
		return &ErrHashMismatch{
			PlanTimestamp: opts.Timestamp,
			Expected:      storedHash,
			Actual:        currentHash,
		}
	}

	logger.Info("Verification passed for plan %s", opts.Timestamp)
	return nil
}

// ---------------------------------------------------------------------------
// Show command options
// ---------------------------------------------------------------------------

// ShowOptions holds configuration for the `todo show` command.
type ShowOptions struct {
	Timestamp string
	PlanDir   string
	FS        FileSystem
}

// Validate checks fields and sets defaults.
func (opts *ShowOptions) Validate() error {
	if opts.Timestamp == "" {
		return ErrMissingTimestamp
	}
	if !isValidTimestamp(opts.Timestamp) {
		return &ErrInvalidTimestamp{Timestamp: opts.Timestamp}
	}
	if opts.PlanDir == "" {
		opts.PlanDir = defaultPlanDir
	}
	if opts.FS == nil {
		opts.FS = defaultFS{}
	}
	return nil
}

// PrintPlan outputs the plan content along with an attestation status header.
func PrintPlan(opts ShowOptions, w io.Writer) error {
	if err := opts.Validate(); err != nil {
		return fmt.Errorf("show options validation: %w", err)
	}

	svc := NewAttestationService(opts.FS)
	attestOpts := AttestOptions{
		Timestamp: opts.Timestamp,
		PlanDir:   opts.PlanDir,
		FS:        opts.FS,
	}

	// Determine attestation status
	var statusMsg string
	err := svc.Verify(attestOpts)
	switch {
	case err == nil:
		statusMsg = "" // no banner needed, everything OK
	case errors.As(err, &ErrAttestationMissing{}):
		statusMsg = fmt.Sprintf("[DRAFT] Plan %s has no attestation hash (never attested)\n", opts.Timestamp)
	case errors.As(err, &ErrHashMismatch{}):
		statusMsg = fmt.Sprintf("[PLAN TAMPERED] Plan %s hash mismatch (content may have changed)\n", opts.Timestamp)
	default:
		// Other error (e.g., plan file missing) – treat as serious
		return fmt.Errorf("verification error: %w", err)
	}

	// Read plan content
	planPath := attestOpts.PlanFilePath()
	planData, err := opts.FS.ReadFile(planPath)
	if err != nil {
		if os.IsNotExist(err) {
			return &ErrPlanNotFound{Path: planPath}
		}
		return fmt.Errorf("reading plan file %q: %w", planPath, err)
	}

	// Write output
	if statusMsg != "" {
		if _, err := fmt.Fprint(w, statusMsg); err != nil {
			return fmt.Errorf("writing status message: %w", err)
		}
	}
	if _, err := w.Write(planData); err != nil {
		return fmt.Errorf("writing plan content: %w", err)
	}

	return nil
}

// ---------------------------------------------------------------------------
// Main CLI
// ---------------------------------------------------------------------------

func main() {
	// Global flags
	planDir := flag.String("d", defaultPlanDir, "Plan directory path")
	verbose := flag.Bool("v", false, "Enable verbose (debug) logging")
	flag.Parse()

	if *verbose {
		logger.SetLevel(LevelDebug)
	}

	args := flag.Args()
	if len(args) < 2 {
		fmt.Fprintf(os.Stderr, "Usage: %s [flags] <command> <timestamp>\nCommands: show, attest\n", os.Args[0])
		flag.PrintDefaults()
		os.Exit(1)
	}

	command := args[0]
	timestamp := args[1]

	fs := defaultFS{}

	switch command {
	case "show":
		opts := ShowOptions{
			Timestamp: timestamp,
			PlanDir:   *planDir,
			FS:        fs,
		}
		if err := PrintPlan(opts, os.Stdout); err != nil {
			logger.Error("Show failed: %v", err)
			os.Exit(1)
		}
	case "attest":
		opts := AttestOptions{
			Timestamp: timestamp,
			PlanDir:   *planDir,
			FS:        fs,
		}
		svc := NewAttestationService(fs)
		if err := svc.Attest(opts); err != nil {
			logger.Error("Attest failed: %v", err)
			os.Exit(1)
		}
		fmt.Printf("Attestation written for plan %s\n", timestamp)
	default:
		fmt.Fprintf(os.Stderr, "Unknown command %q. Supported: show, attest\n", command)
		os.Exit(1)
	}
}