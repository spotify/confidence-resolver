package confidence

import "testing"

// newProviderWithOptions builds a provider with no live dependencies. Only the
// option plumbing is under test here, and the constructor touches its
// dependencies lazily, so nils are safe.
func newProviderWithOptions(t *testing.T, opts ...Option) *LocalResolverProvider {
	t.Helper()
	return NewLocalResolverProvider(nil, nil, nil, "unit-test-secret", nil, opts...)
}

// Dedup is on by default, so a caller that never mentions it gets it.
func TestApplyDedupIsOnByDefault(t *testing.T) {
	p := newProviderWithOptions(t)
	if !p.enableApplyDedup {
		t.Error("apply dedup must be enabled when no option is supplied; it is on by default")
	}
}

// WithEnableApplyDedup predates the default flip and is still exported, so code
// written against the previous release must keep working — and must still end up
// with dedup ENABLED, not merely compile.
func TestWithEnableApplyDedupStillEnables(t *testing.T) {
	p := newProviderWithOptions(t, WithEnableApplyDedup())
	if !p.enableApplyDedup {
		t.Error("WithEnableApplyDedup must still enable dedup for callers written against the previous release")
	}
}

func TestWithDisableApplyDedupTurnsItOff(t *testing.T) {
	p := newProviderWithOptions(t, WithDisableApplyDedup())
	if p.enableApplyDedup {
		t.Error("WithDisableApplyDedup must turn dedup off; the opt-out is the only way to get the unfiltered apply stream")
	}
}

// Disable wins over the legacy enable, as documented on ProviderConfig.
func TestDisableApplyDedupTakesPrecedenceOverEnable(t *testing.T) {
	p := newProviderWithOptions(t, WithEnableApplyDedup(), WithDisableApplyDedup())
	if p.enableApplyDedup {
		t.Error("WithDisableApplyDedup must take precedence over WithEnableApplyDedup")
	}
}

// The ProviderConfig path is what existing users construct. These assert the
// config-to-option translation NewProvider performs, so a regression there is
// caught rather than only the Option layer being covered.
func TestProviderConfigApplyDedupTranslation(t *testing.T) {
	cases := []struct {
		name       string
		config     ProviderConfig
		wantDedup  bool
		wantReason string
	}{
		{
			name:       "unset leaves dedup on",
			config:     ProviderConfig{},
			wantDedup:  true,
			wantReason: "a config that never mentions dedup must get the default, which is on",
		},
		{
			name:       "legacy EnableApplyDedup true keeps dedup on",
			config:     ProviderConfig{EnableApplyDedup: true},
			wantDedup:  true,
			wantReason: "existing callers set EnableApplyDedup: true and must still get dedup",
		},
		{
			name:       "legacy EnableApplyDedup false does not disable",
			config:     ProviderConfig{EnableApplyDedup: false},
			wantDedup:  true,
			wantReason: "false is the zero value and indistinguishable from unset, so it cannot mean off; DisableApplyDedup is the opt-out",
		},
		{
			name:       "DisableApplyDedup turns dedup off",
			config:     ProviderConfig{DisableApplyDedup: true},
			wantDedup:  false,
			wantReason: "DisableApplyDedup is the documented opt-out",
		},
		{
			name:       "DisableApplyDedup beats EnableApplyDedup",
			config:     ProviderConfig{EnableApplyDedup: true, DisableApplyDedup: true},
			wantDedup:  false,
			wantReason: "DisableApplyDedup is documented to take precedence",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			// Mirrors the call NewProvider makes; NewProvider itself needs live
			// state and network, so the translation is exercised here instead.
			opts := buildProviderOptions(
				tc.config.StatePollInterval,
				tc.config.LogPollInterval,
				tc.config.DisableApplyDedup,
				tc.config.DisableExposureCollection,
			)
			p := newProviderWithOptions(t, opts...)
			if p.enableApplyDedup != tc.wantDedup {
				t.Errorf("dedup enabled = %v, want %v: %s", p.enableApplyDedup, tc.wantDedup, tc.wantReason)
			}
		})
	}
}

// The test provider defaults to dedup on as well, so a test harness behaves
// like production unless it opts out.
func TestProviderTestConfigApplyDedupTranslation(t *testing.T) {
	for _, tc := range []struct {
		name      string
		config    ProviderTestConfig
		wantDedup bool
	}{
		{"unset leaves dedup on", ProviderTestConfig{}, true},
		{"DisableApplyDedup turns dedup off", ProviderTestConfig{DisableApplyDedup: true}, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			opts := buildProviderOptions(
				tc.config.StatePollInterval,
				tc.config.LogPollInterval,
				tc.config.DisableApplyDedup,
				tc.config.DisableExposureCollection,
			)
			p := newProviderWithOptions(t, opts...)
			if p.enableApplyDedup != tc.wantDedup {
				t.Errorf("dedup enabled = %v, want %v", p.enableApplyDedup, tc.wantDedup)
			}
		})
	}
}
