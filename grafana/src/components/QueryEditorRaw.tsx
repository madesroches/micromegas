import {css} from '@emotion/css'
import React, {useState} from 'react'
import {useMeasure} from 'react-use'
import {GrafanaTheme2} from '@grafana/data'
import {Modal, useStyles2} from '@grafana/ui'
import AutoSizer from 'react-virtualized-auto-sizer'

import {RawEditor} from './RawEditor'
import {QueryTool} from './QueryTool'

export function QueryEditorRaw({onChange, query, editorLanguageDefinition}: any) {
  const styles = useStyles2(getStyles)
  const [isExpanded, setIsExpanded] = useState(false)
  const [toolboxRef, toolboxMeasure] = useMeasure<HTMLDivElement>()

  const renderQueryEditor = (width?: number, height?: number) => {
    return (
      <RawEditor
        editorLanguageDefinition={editorLanguageDefinition}
        query={query}
        width={width}
        height={height ? height - toolboxMeasure.height : undefined}
        onChange={onChange}
      >
        {({formatQuery}: any) => {
          return (
            <div ref={toolboxRef}>
              <QueryTool onFormatCode={formatQuery} showTools onExpand={setIsExpanded} isExpanded={isExpanded} />
            </div>
          )
        }}
      </RawEditor>
    )
  }

  const renderEditor = (standalone = false) => {
    return standalone ? (
      <AutoSizer>
        {({width, height}) => {
          return renderQueryEditor(width, height)
        }}
      </AutoSizer>
    ) : (
      <div>{renderQueryEditor()}</div>
    )
  }
  return (
    <>
      {isExpanded ? (
        <Modal
          title={`Query ${query.refId}`}
          closeOnBackdropClick={false}
          closeOnEscape={false}
          className={styles.modal}
          contentClassName={styles.modalContent}
          isOpen={isExpanded}
          onDismiss={() => {
            setIsExpanded(false)
          }}
        >
          {renderEditor(true)}
        </Modal>
      ) : (
        renderQueryEditor()
      )}
    </>
  )
}

function getStyles(theme: GrafanaTheme2) {
  return {
    modal: css`
      width: 95vw;
      height: 95vh;
    `,
    modalContent: css`
      height: 100%;
      padding-top: 0;
    `,
  }
}
