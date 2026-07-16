import { render } from '@testing-library/react'
import { DataType } from 'apache-arrow'
import { renderLogColumn, type LogColumn } from '../log-utils'

function renderCol(col: LogColumn, row: Record<string, unknown>, wrap?: boolean) {
  return render(<>{renderLogColumn(col, row, { wrap })}</>)
}

describe('renderLogColumn wrap behavior', () => {
  it('emits whitespace-pre-wrap break-words for the generic/default column when wrap is true', () => {
    const col: LogColumn = { name: 'msg', kind: 'generic', type: new DataType() }
    const { container } = renderCol(col, { msg: 'a long message\nwith a newline' }, true)
    const span = container.querySelector('span')
    expect(span).not.toBeNull()
    expect(span?.className).toContain('whitespace-pre-wrap')
    expect(span?.className).toContain('break-words')
    expect(span?.className).not.toContain('truncate')
  })

  it('emits truncate for the generic/default column when wrap is false', () => {
    const col: LogColumn = { name: 'msg', kind: 'generic', type: new DataType() }
    const { container } = renderCol(col, { msg: 'a long message' }, false)
    const span = container.querySelector('span')
    expect(span?.className).toContain('truncate')
    expect(span?.className).not.toContain('whitespace-pre-wrap')
  })

  it('emits truncate for the generic/default column when wrap is absent', () => {
    const col: LogColumn = { name: 'msg', kind: 'generic', type: new DataType() }
    const { container } = renderCol(col, { msg: 'a long message' }, undefined)
    const span = container.querySelector('span')
    expect(span?.className).toContain('truncate')
    expect(span?.className).not.toContain('whitespace-pre-wrap')
  })

  it('emits whitespace-pre-wrap break-words for the target column when wrap is true', () => {
    const col: LogColumn = { name: 'target', kind: 'target', type: new DataType() }
    const { container } = renderCol(col, { target: 'my::module::path' }, true)
    const span = container.querySelector('span')
    expect(span?.className).toContain('whitespace-pre-wrap')
    expect(span?.className).toContain('break-words')
  })

  it('emits truncate for the target column when wrap is false', () => {
    const col: LogColumn = { name: 'target', kind: 'target', type: new DataType() }
    const { container } = renderCol(col, { target: 'my::module::path' }, false)
    const span = container.querySelector('span')
    expect(span?.className).toContain('truncate')
  })
})
