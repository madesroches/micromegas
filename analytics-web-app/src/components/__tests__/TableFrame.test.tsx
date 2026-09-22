import { render, screen } from '@testing-library/react'
import { TableFrame } from '../TableFrame'

describe('TableFrame', () => {
  it('renders children inside an overflow-auto scroller, with the outer frame overflow-hidden and not flex-1', () => {
    const { container } = render(
      <TableFrame>
        <div data-testid="table-content">rows</div>
      </TableFrame>
    )

    const frame = container.firstChild as HTMLElement
    expect(frame.className).toContain('overflow-hidden')
    expect(frame.className).not.toContain('flex-1')

    const content = screen.getByTestId('table-content')
    const scroller = content.parentElement as HTMLElement
    expect(scroller.className).toContain('overflow-auto')
    expect(scroller.parentElement).toBe(frame)
  })

  it('renders the footer as a sibling of the scroll container, not inside it', () => {
    const { container } = render(
      <TableFrame footer={<div data-testid="footer">pagination</div>}>
        <div data-testid="table-content">rows</div>
      </TableFrame>
    )

    const frame = container.firstChild as HTMLElement
    const footer = screen.getByTestId('footer')
    const content = screen.getByTestId('table-content')

    expect(footer.parentElement).toBe(frame)
    expect(content.parentElement).not.toBe(frame)
  })

  it('renders nothing extra when footer is omitted', () => {
    const { container } = render(
      <TableFrame>
        <div data-testid="table-content">rows</div>
      </TableFrame>
    )

    const frame = container.firstChild as HTMLElement
    // Only the scroller div is a child of the frame; no extra footer node.
    expect(frame.children.length).toBe(1)
  })
})
