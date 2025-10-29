import React from 'react'

import {HorizontalGroup, IconButton, Tooltip, Icon} from '@grafana/ui'

interface QueryToolProps {
  showTools?: boolean
  isExpanded?: boolean
  onFormatCode?: () => void
  onExpand?: (expand: boolean) => void
}

export const QueryTool = ({onFormatCode, onExpand, isExpanded}: QueryToolProps) => (
  <div className="none" style={{padding: '5px 0'}}>
    <div>
      <HorizontalGroup spacing="sm">
        {onFormatCode && <IconButton onClick={onFormatCode} name="brackets-curly" size="xs" tooltip="Format query" />}
        {onExpand && (
          <IconButton
            onClick={() => onExpand(!isExpanded)}
            name={isExpanded ? 'angle-up' : 'angle-down'}
            size="xs"
            tooltip={isExpanded ? 'Collapse editor' : 'Expand editor'}
          />
        )}
        <Tooltip content="Hit CMD+Return to run query">
          <Icon className="hint" name="keyboard" />
        </Tooltip>
      </HorizontalGroup>
    </div>
  </div>
)
