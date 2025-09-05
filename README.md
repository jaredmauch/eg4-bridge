# eg4-bridge

eg4-bridge is a tool for monitoring and controlling EG4 inverters locally. It is based on the work originally done by @Chris Elsworth for LuxPower inverters.

It allows you to monitor and possibly control your inverter locally.

## Database Support

eg4-bridge supports multiple database backends for storing inverter data:

- **PostgreSQL** - Recommended for production use
- **MySQL** - Alternative production database
- **SQLite** - Lightweight option for development and testing

Database migrations are automatically applied on startup.

### PostgreSQL Connection Methods

eg4-bridge supports multiple PostgreSQL connection methods:

#### Localhost Connection
```yaml
databases:
- enabled: true
  url: postgres://username@localhost/database_name
  # With password: postgres://username:password@localhost:5432/database_name
```

#### Unix Socket Connection
**Note**: Unix socket connections require URL validation improvements. For now, use localhost connections.

```yaml
databases:
- enabled: true
  # This format is valid for PostgreSQL but needs URL validation fix:
  url: postgres://username@/database_name?host=/var/run/postgresql
  # Custom socket path: postgres://username@/database_name?host=/tmp
```

**Workaround**: Use localhost with Unix socket by configuring PostgreSQL to listen on localhost:
```yaml
databases:
- enabled: true
  url: postgres://username@localhost/database_name
```

#### Trust Authentication (No Password)
When using trust authentication (common for localhost connections):
```yaml
databases:
- enabled: true
  url: postgres://user@localhost/eg4_bridge
```

See `config.yaml.example` for complete database configuration options.

## Home Assistant add-on (UNMAINTAINED)
Click the icon below to add this repository to your Home Assistant instance or follow the procedure highlighted on the [Home Assistant website](https://home-assistant.io/hassio/installing_third_party_addons).

I don't use home assistant, but the original implemention by @celsworth included it, so it should be easy to revive.  My main focus is on sending data to influx v1

## Pre-built images (HOME ASSISTANT UNMAINTAINED)

## Documentation

I am attempting to provide a more stable set of documentation with the EG4 brand devices, which are (were?) compatible with the LuxPower inverters.  It may also work for other inverters.

The tests are still from the original version and I expect will be revived, help with maintaining this is welcome.

## Pull requests

Issues and pull requests are welcome, and co-maintainers will be considered if you send a PR.

It is always helpful if you provide details about what device and configuration you are using.

