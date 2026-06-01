package offload

import (
	"context"
	"encoding/json"
	"sync"
)

// SpecFetcher is what the cache calls when it doesn't yet have an evaluator
// for a given run_id. Typically wired to read `runs.assert_spec` from Postgres
// and unmarshal into a Spec.
type SpecFetcher func(ctx context.Context, runID string) (Spec, error)

// Cache memoizes per-run Evaluators. Specs don't change after run creation so
// this never invalidates - it just grows as more distinct run_ids show up.
// Safe for concurrent use.
type Cache struct {
	mu     sync.RWMutex
	entries map[string]*entry
	fetch  SpecFetcher
}

type entry struct {
	eval    *Evaluator
	noRules bool // shortcut so we don't re-build for runs without any offload tier
}

func NewCache(fetch SpecFetcher) *Cache {
	return &Cache{entries: map[string]*entry{}, fetch: fetch}
}

// Get returns an Evaluator for runID. (nil, nil) means "no offload tier is
// configured for this run" - callers should skip evaluation in that case.
func (c *Cache) Get(ctx context.Context, runID string) (*Evaluator, error) {
	c.mu.RLock()
	e, ok := c.entries[runID]
	c.mu.RUnlock()
	if ok {
		if e.noRules {
			return nil, nil
		}
		return e.eval, nil
	}

	spec, err := c.fetch(ctx, runID)
	if err != nil {
		return nil, err
	}

	if !spec.HasAnyRule() {
		c.mu.Lock()
		c.entries[runID] = &entry{noRules: true}
		c.mu.Unlock()
		return nil, nil
	}

	eval, err := NewEvaluator(spec)
	if err != nil {
		return nil, err
	}
	c.mu.Lock()
	c.entries[runID] = &entry{eval: eval}
	c.mu.Unlock()
	return eval, nil
}

// ParseSpecFromAssert extracts the offload-tier fields from the raw assert
// JSONB that came in on POST /v1/runs. Unknown fields are ignored by encoding/json.
func ParseSpecFromAssert(raw json.RawMessage) (Spec, error) {
	if len(raw) == 0 {
		return Spec{}, nil
	}
	var spec Spec
	if err := json.Unmarshal(raw, &spec); err != nil {
		return Spec{}, err
	}
	return spec, nil
}
