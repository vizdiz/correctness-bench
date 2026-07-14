import type { ReactNode } from 'react'
import { NavLink } from 'react-router-dom'
import { cn } from '../lib/cn'
import { ThemeToggle } from './ui/ThemeToggle'

function NavItem({ to, label, end }: { to: string; label: string; end?: boolean }) {
  return (
    <NavLink
      to={to}
      end={end}
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
      <aside className="sticky top-0 flex h-screen w-56 shrink-0 flex-col gap-6 overflow-y-auto border-r border-border bg-surface px-4 py-5">
        <div className="flex items-center gap-2 px-2">
          {/* Wordmark - the cliff motif: correctness (forest) drops past a flat
              latency line (graphite). Matches the favicon. */}
          <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden>
            <path d="M2 7 H20" fill="none" stroke="var(--color-latency)" strokeWidth="2" strokeLinecap="round" opacity="0.9" />
            <path
              d="M2 7 H11 V17 H20"
              fill="none"
              stroke="var(--color-correct)"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
          <span className="text-sm font-semibold tracking-tight">correctness-bench</span>
        </div>
        <nav className="flex flex-col gap-0.5">
          <NavItem to="/runs" label="Runs" end />
          <NavItem to="/runs/new" label="New run" />
          <NavItem to="/templates" label="Templates" />
        </nav>
        <div className="mt-auto px-2">
          <ThemeToggle />
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <main className="mx-auto w-full max-w-5xl flex-1 px-8 py-8">{children}</main>
      </div>
    </div>
  )
}
