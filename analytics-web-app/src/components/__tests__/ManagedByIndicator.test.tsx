import { render, screen, fireEvent } from '@testing-library/react'
import { ManagedByIndicator } from '../ManagedByIndicator'

const SOURCE = 'https://example.com/repo/screen.json'
const trigger = () => screen.getByRole('button', { name: /managed by source control/i })

describe('ManagedByIndicator', () => {
  it('is closed initially', () => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    expect(screen.queryByText(/View source/)).not.toBeInTheDocument()
  })

  it.each([
    ['click', () => fireEvent.click(trigger())],
    ['mouseEnter', () => fireEvent.mouseEnter(trigger())],
    ['focus', () => fireEvent.focus(trigger())],
  ])('opens on %s with a link to the source', (_name, act) => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    act()
    expect(screen.getByRole('link', { name: /View source/ })).toHaveAttribute('href', SOURCE)
    expect(screen.getByText(/managed by source control/i, { selector: 'p' })).toBeInTheDocument()
  })

  it('closes on Escape', () => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    fireEvent.click(trigger())
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByText(/View source/)).not.toBeInTheDocument()
  })

  it('closes on outside mousedown', () => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    fireEvent.click(trigger())
    fireEvent.mouseDown(document.body)
    expect(screen.queryByText(/View source/)).not.toBeInTheDocument()
  })

  it('stays open on mousedown inside the popover', () => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    fireEvent.click(trigger())
    fireEvent.mouseDown(screen.getByRole('link', { name: /View source/ }))
    expect(screen.getByRole('link', { name: /View source/ })).toBeInTheDocument()
  })

  it('closes on mouse leave', () => {
    const { container } = render(<ManagedByIndicator managedBy={SOURCE} />)
    fireEvent.mouseEnter(trigger())
    fireEvent.mouseLeave(container.firstElementChild!)
    expect(screen.queryByText(/View source/)).not.toBeInTheDocument()
  })

  it('closes when focus leaves the container', () => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    fireEvent.focus(trigger())
    fireEvent.blur(trigger(), { relatedTarget: document.body })
    expect(screen.queryByText(/View source/)).not.toBeInTheDocument()
  })

  it('stays open when focus moves to the source link', () => {
    render(<ManagedByIndicator managedBy={SOURCE} />)
    fireEvent.focus(trigger())
    fireEvent.blur(trigger(), { relatedTarget: screen.getByRole('link', { name: /View source/ }) })
    expect(screen.getByRole('link', { name: /View source/ })).toBeInTheDocument()
  })
})
