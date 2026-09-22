interface TableFrameProps {
  children: React.ReactNode // the <table>
  footer?: React.ReactNode // pinned below the scroll area (e.g. pagination)
}

export function TableFrame({ children, footer }: TableFrameProps) {
  return (
    <div className="flex flex-col border border-theme-border rounded-lg overflow-hidden">
      <div className="overflow-auto">{children}</div>
      {footer}
    </div>
  )
}
