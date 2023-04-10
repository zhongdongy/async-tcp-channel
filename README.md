### Loggers

The following loggers are used:

- `atc-listener`: Server side logs.
- `atc-connector`: Client side logs.
- `atc-frame`: Frame parsing logs.

Example `log4rs.yml` configuration file:

```yaml
refresh_rate: 30 seconds

appenders:
  atc_listener_appender:
    kind: file
    path: "log/atc_listener.log"
    encoder:
      pattern: "{d} {l} - {m}{n}"
  atc_connector_appender:
    kind: file
    path: "log/atc_connector.log"
    encoder:
      pattern: "{d} {l} - {m}{n}"
  atc_frame_appender:
    kind: file
    path: "log/atc_frame.log"
    encoder:
      pattern: "{d} {l} - {m}{n}"
 

root:
  level: warn
  appenders:
    - atc_listener_appender

loggers:
  atc-listener:
    level: debug
    appenders:
      - atc_listener_appender
    additive: false
  atc-connector:
    level: debug
    appenders:
      - atc_connector_appender
    additive: false
  atc-frame:
    level: debug
    appenders:
      - atc_frame_appender
    additive: false

```