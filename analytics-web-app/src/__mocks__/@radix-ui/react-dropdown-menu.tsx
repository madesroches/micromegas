/**
 * Test mock for @radix-ui/react-dropdown-menu.
 * Renders trigger and content inline (no portal) so items are queryable in tests.
 */
import { ReactNode } from 'react'

export const Root = ({ children }: { children: ReactNode }) => <div>{children}</div>
export const Trigger = ({ children }: { children: ReactNode }) => <>{children}</>
export const Portal = ({ children }: { children: ReactNode }) => <>{children}</>
export const Content = ({ children }: { children: ReactNode }) => <div>{children}</div>
export const Item = ({
  children,
  onSelect,
  ...props
}: { children: ReactNode; onSelect?: () => void } & Record<string, unknown>) => (
  <button {...props} onClick={onSelect}>{children}</button>
)
export const Separator = (props: Record<string, unknown>) => <hr {...props} />
