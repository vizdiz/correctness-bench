package api

import (
	"regexp"
	"strings"
)

// Redacted is the placeholder stored in place of any secret-bearing header value.
const Redacted = "***"

// authHeaderNames are headers whose VALUES are always treated as secrets and
// redacted regardless of content (case-insensitive match on the header name).
var authHeaderNames = map[string]struct{}{
	"authorization":        {},
	"proxy-authorization":  {},
	"x-api-key":            {},
	"api-key":              {},
	"apikey":               {},
	"x-auth-token":         {},
	"x-amz-security-token": {},
	"x-goog-api-key":       {},
	"cookie":               {},
	"set-cookie":           {},
	"x-csrf-token":         {},
}

// secretishValue catches token-shaped values in non-auth-named headers: bearer/
// basic schemes, common key prefixes, or a long opaque token-like string. This
// is a defense-in-depth heuristic, NOT keyed to any specific canary string.
var secretishValue = regexp.MustCompile(`(?i)(bearer\s|basic\s|^sk-|^pk-|^ghp_|^xox[baprs]-|[A-Za-z0-9_\-]{20,})`)

// ScrubHeaders returns a copy of headers safe to persist: any value that looks
// like a credential is replaced with Redacted. Non-secret values (e.g.
// Content-Type) are preserved so the redacted spec is still re-runnable.
func ScrubHeaders(headers map[string]string) map[string]string {
	if headers == nil {
		return map[string]string{}
	}
	out := make(map[string]string, len(headers))
	for k, v := range headers {
		if isSecretHeader(k, v) {
			out[k] = Redacted
		} else {
			out[k] = v
		}
	}
	return out
}

func isSecretHeader(name, value string) bool {
	if _, ok := authHeaderNames[strings.ToLower(strings.TrimSpace(name))]; ok {
		return true
	}
	return secretishValue.MatchString(value)
}
