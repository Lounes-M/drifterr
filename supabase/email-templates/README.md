# Branded auth emails

On-brand HTML for Supabase's transactional auth emails (sent via the configured
SMTP — Resend). Dark theme, gradient CTA, logo tile, matching the site.

## Apply them

Supabase → **Authentication → Emails → Templates**. For each template, paste the
matching file's contents into the **Message body (HTML)** and set the subject:

| Supabase template      | File                  | Suggested subject              |
| ---------------------- | --------------------- | ------------------------------ |
| Confirm signup         | `confirm-signup.html` | `Confirm your Drifterr email`  |
| Magic Link             | `magic-link.html`     | `Your Drifterr sign-in link`   |
| Reset Password         | `reset-password.html` | `Reset your Drifterr password` |
| Change Email Address   | `change-email.html`   | `Confirm your new email`       |

## Notes
- All use `{{ .ConfirmationURL }}` (Supabase fills it at send time).
- Email-client-safe: table layout, inline styles, system font stack (web fonts
  are unreliable in mail clients), and a solid `bgcolor` fallback under the
  gradient button for Outlook.
- The logo is loaded from `https://drifterr.app/assets/favicon/favicon-48.png`,
  so it resolves once the site is deployed.
