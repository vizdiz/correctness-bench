import { useEffect, useState } from 'react'

type Theme = 'light' | 'dark'

function current(): Theme {
  const t = document.documentElement.getAttribute('data-theme')
  return t === 'dark' ? 'dark' : 'light'
}

/**
 * Flips <html data-theme> between light and dark and remembers the choice.
 * The no-flash default is set in index.html before first paint; this just
 * toggles + persists at runtime.
 */
export function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(current)

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme)
    try {
      localStorage.setItem('theme', theme)
    } catch {
      /* private mode - ignore */
    }
  }, [theme])

  const next = theme === 'dark' ? 'light' : 'dark'
  return (
    <button
      type="button"
      onClick={() => setTheme(next)}
      aria-label={`Switch to ${next} mode`}
      title={`Switch to ${next} mode`}
      className="flex items-center gap-2 rounded-[var(--radius)] border border-border px-2.5 py-1.5 text-xs text-text-muted transition-colors hover:border-border-strong hover:text-text"
    >
      {theme === 'dark' ? (
        // Sun - click to go light
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden>
          <circle cx="12" cy="12" r="4" />
          <path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5L19 19M19 5l-1.5 1.5M6.5 17.5L5 19" />
        </svg>
      ) : (
        // Moon - click to go dark
        <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
          <path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" />
        </svg>
      )}
      <span>{theme === 'dark' ? 'Light' : 'Dark'}</span>
    </button>
  )
}
