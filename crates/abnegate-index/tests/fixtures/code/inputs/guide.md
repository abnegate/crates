# Widgets guide

## Installing

Run `npm install widgets` and import the components you need.

```js
import { CartBadge } from 'widgets';
```

## Configuring

Create `widgets.config.json` next to your package manifest. Every key is optional:

- `retries`: how many times a request is retried.
- `verbose`: log every request.
- `outputDirectory`: where builds go.

## Troubleshooting

If the badge never updates, check that `count` is a number. Strings are ignored.
