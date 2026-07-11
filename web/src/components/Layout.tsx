import type { ReactNode } from 'react'
import { NavLink } from 'react-router-dom'
import { cn } from '../lib/cn'

function NavItem({ to, label }: { to: string; label: string }) {
  return (
    <NavLink
      to={to}
      end={to === '/'}
      className={({ isActive }) =>
        cn(
          'block rounded-[var(--radius)] px-3 py-1.5 text-sm transition-colors',
          isActive ? 'bg-surface-2 text-text' : 'text-text-muted hover:text-text hover:bg-surface-2/60',
        )
      }
    >
      {label}
    </NavLink>
  )
}

export function Layout({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-full">
      <aside className="flex w-56 shrink-0 flex-col gap-6 border-r border-border bg-surface px-4 py-5">
        <div className="flex items-center gap-2 px-2">
          {/* Wordmark — the cliff motif: a flat line that drops. */}
          <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden>
            <path
              d="M2 7 H11 V17 H20"
              fill="none"
              stroke="var(--color-correct)"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
            <path d="M2 7 H20" fill="none" stroke="var(--color-accent)" strokeWidth="2" strokeLinecap="round" opacity="0.5" />
          </svg>
          <span className="text-sm font-semibold tracking-tight">correctness-bench</span>
        </div>
        <nav className="flex flex-col gap-0.5">
          <NavItem to="/runs" label="Runs" />
          <NavItem to="/runs/new" label="New run" />
          <NavItem to="/templates" label="Templates" />
        </nav>
        <div className="mt-auto px-2 text-xs leading-relaxed text-text-faint">
          Correctness as a function of load. The cliff is the headline.
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <main className="mx-auto w-full max-w-5xl flex-1 px-8 py-8">{children}</main>
      </div>
    </div>
  )
}
