import React, {useEffect, useState} from 'react'
import {InlineSwitch, FieldSet, InlineField, SecretInput, Input, Select, InlineFieldRow, InlineLabel} from '@grafana/ui'
import {DataSourcePluginOptionsEditorProps, SelectableValue} from '@grafana/data'
import {FlightSQLDataSourceOptions, authTypeOptions, SecureJsonData} from '../types'
import {
  onHostChange,
  onTokenChange,
  onSecureChange,
  onUsernameChange,
  onPasswordChange,
  onAuthTypeChange,
  onKeyChange,
  onValueChange,
  addMetaData,
  removeMetaData,
  onResetToken,
  onResetPassword,
  onOAuthIssuerChange,
  onOAuthClientIdChange,
  onOAuthClientSecretChange,
  onOAuthAudienceChange,
  onResetOAuthClientSecret,
  onEnableUserAttributionChange,
} from './utils'

export function ConfigEditor(props: DataSourcePluginOptionsEditorProps<FlightSQLDataSourceOptions, SecureJsonData>) {
  const {options, onOptionsChange} = props
  const {jsonData} = options
  const {secureJsonData, secureJsonFields} = options

  const [selectedAuthType, setAuthType] = useState<SelectableValue<string>>({
    value: jsonData?.selectedAuthType,
    label: jsonData?.selectedAuthType,
  })
  const existingMetastate = jsonData?.metadata?.length && jsonData?.metadata?.map((m: any) => ({key: Object.keys(m)[0], value: Object.values(m)[0]}))
  const [metaDataArr, setMetaData] = useState(existingMetastate || [{key: '', value: ''}])
  useEffect(() => {
    onAuthTypeChange(selectedAuthType, options, onOptionsChange)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedAuthType])

  useEffect(() => {
    const {onOptionsChange, options} = props  
      const mapData = metaDataArr?.map((m: any) => ({[m.key]: m.value}))
        const jsonData = {
        ...options.jsonData,
        metadata: mapData,
      }
      onOptionsChange({...options, jsonData})
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [metaDataArr])

  return (
    <div>
      <FieldSet label="FlightSQL Connection" width={400}>
        <InlineField labelWidth={20} label="Host:Port">
          <Input
            width={40}
            name="host"
            type="text"
            value={jsonData.host || ''}
            placeholder="localhost:1234"
            onChange={(e) => onHostChange(e, options, onOptionsChange)}
          ></Input>
        </InlineField>
        <InlineField labelWidth={20} label="Auth Type">
          <Select
            options={authTypeOptions}
            onChange={setAuthType}
            value={selectedAuthType || ''}
            allowCustomValue={true}
            width={40}
            placeholder="token"
          />
        </InlineField>
        {selectedAuthType?.label === 'token' && (
          <InlineField labelWidth={20} label="Token">
            <SecretInput
              width={40}
              name="token"
              type="text"
              value={secureJsonData?.token || ''}
              placeholder="****************"
              onChange={(e) => onTokenChange(e, options, onOptionsChange)}
              onReset={() => onResetToken(options, onOptionsChange)}
              isConfigured={secureJsonFields?.token}
            ></SecretInput>
          </InlineField>
        )}
        {selectedAuthType?.label === 'username/password' && (
          <InlineFieldRow style={{flexFlow: 'row'}}>
            <InlineField labelWidth={20} label="Username">
              <Input
                width={40}
                name="username"
                type="text"
                placeholder="username"
                onChange={(e) => onUsernameChange(e, options, onOptionsChange)}
                value={jsonData.username || ''}
              ></Input>
            </InlineField>
            <InlineField labelWidth={20} label="Password">
              <SecretInput
                width={40}
                name="password"
                type="text"
                value={secureJsonData?.password || ''}
                placeholder="****************"
                onChange={(e) => onPasswordChange(e, options, onOptionsChange)}
                onReset={() => onResetPassword(options, onOptionsChange)}
                isConfigured={secureJsonFields?.password}
              ></SecretInput>
            </InlineField>
          </InlineFieldRow>
        )}
        {selectedAuthType?.value === 'oauth2' && (
          <>
            <InlineField
              labelWidth={20}
              label="OIDC Issuer"
              tooltip="Identity provider URL (e.g., https://accounts.google.com)"
            >
              <Input
                width={40}
                name="oauthIssuer"
                type="text"
                value={jsonData.oauthIssuer || ''}
                placeholder="https://accounts.google.com"
                onChange={(e) => onOAuthIssuerChange(e, options, onOptionsChange)}
              />
            </InlineField>

            <InlineField labelWidth={20} label="Client ID">
              <Input
                width={40}
                name="oauthClientId"
                type="text"
                value={jsonData.oauthClientId || ''}
                placeholder="service@project.iam.gserviceaccount.com"
                onChange={(e) => onOAuthClientIdChange(e, options, onOptionsChange)}
              />
            </InlineField>

            <InlineField labelWidth={20} label="Client Secret">
              <SecretInput
                width={40}
                name="oauthClientSecret"
                type="text"
                value={secureJsonData?.oauthClientSecret || ''}
                placeholder="****************"
                onChange={(e) => onOAuthClientSecretChange(e, options, onOptionsChange)}
                onReset={() => onResetOAuthClientSecret(options, onOptionsChange)}
                isConfigured={secureJsonFields?.oauthClientSecret}
              />
            </InlineField>

            <InlineField
              labelWidth={20}
              label="Audience (optional)"
              tooltip="Required for Auth0 and Azure AD"
            >
              <Input
                width={40}
                name="oauthAudience"
                type="text"
                value={jsonData.oauthAudience || ''}
                placeholder="https://api.micromegas.example.com"
                onChange={(e) => onOAuthAudienceChange(e, options, onOptionsChange)}
              />
            </InlineField>

            <InlineFieldRow>
              <InlineField>
                <span className="help-text">
                  OAuth 2.0 client credentials flow for service accounts.
                  Credentials managed by identity provider (Google, Auth0, Azure AD, Okta).
                </span>
              </InlineField>
            </InlineFieldRow>
          </>
        )}

        <InlineField labelWidth={20} label="Require TLS / SSL">
          <InlineSwitch
            label=""
            value={jsonData.secure}
            onChange={() => onSecureChange(options, onOptionsChange)}
            showLabel={false}
            disabled={false}
          />
        </InlineField>
      </FieldSet>
      <FieldSet label="Privacy Settings" width={400}>
        <InlineField
          labelWidth={20}
          label="Enable User Attribution"
          tooltip="Send user identity (username, email) to FlightSQL server for audit logging. Disable for GDPR compliance if needed."
        >
          <InlineSwitch
            label=""
            value={jsonData.enableUserAttribution !== false}
            onChange={() => onEnableUserAttributionChange(options, onOptionsChange)}
            showLabel={false}
            disabled={false}
          />
        </InlineField>
        <InlineFieldRow>
          <InlineField>
            <span className="help-text">
              When enabled (default), Grafana user information is sent to the FlightSQL server for audit logging and attribution.
              This helps track which users are running queries. Disable if GDPR or privacy policies prohibit sending user data.
            </span>
          </InlineField>
        </InlineFieldRow>
      </FieldSet>
      <FieldSet label="MetaData" width={400}>
        {metaDataArr?.map((_: any, i: any) => (
          <InlineFieldRow key={i} style={{flexFlow: 'row'}}>
            <InlineField labelWidth={20} label="Key">
              <Input
                key={i}
                width={40}
                name="key"
                type="text"
                value={metaDataArr[i]?.key || ''}
                placeholder="key"
                onChange={(e) => onKeyChange(e, metaDataArr, i, setMetaData)}
              ></Input>
            </InlineField>
            <InlineField labelWidth={20} label="Value">
              <Input
                key={i}
                width={40}
                name="value"
                type="text"
                value={metaDataArr[i]?.value || ''}
                placeholder="value"
                onChange={(e) => onValueChange(e, metaDataArr, i, setMetaData)}
              ></Input>
            </InlineField>
            {i + 1 >= metaDataArr.length && (
              <InlineLabel as="button" className="" onClick={() => addMetaData(setMetaData, metaDataArr)} width="auto">
                +
              </InlineLabel>
            )}
            {i > 0 && (
              <InlineLabel
                as="button"
                className=""
                width="auto"
                onClick={() => removeMetaData(i, setMetaData, metaDataArr)}
              >
                -
              </InlineLabel>
            )}
          </InlineFieldRow>
        ))}
      </FieldSet>
    </div>
  )
}
