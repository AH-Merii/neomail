//! The setup wizards for the OAuth providers, as data: Gmail with your own Google OAuth client
//! (issue #47) and Outlook with your own Microsoft Entra app (#48). `neomail account add` walks
//! them with [`crate::wizard`]; the checks and the write are in `crate::account::add`.
//!
//! Console labels drift, so each step quotes its provider briefly and links the page.

use crate::wizard::{Cleaned, Kind, Mistake, Question, Step, Wizard};

// What a failed check can mean: the `kind` of a `CheckError`, matched in the tables' mistakes.
pub const NAME_TAKEN: &str = "name-taken";
pub const UNKNOWN_ID: &str = "unknown-id";
pub const WRONG_SECRET: &str = "wrong-secret";
pub const BAD_CLIENT: &str = "bad-client";
pub const NOT_MULTITENANT: &str = "not-multitenant";
pub const WORK_ONLY: &str = "work-only";
pub const TIMED_OUT: &str = "timed-out";
pub const DECLINED: &str = "declined";
pub const NO_MAIL: &str = "no-mail";
pub const WRONG_ACCOUNT: &str = "wrong-account";
pub const REDIRECT_MISMATCH: &str = "redirect-mismatch";
pub const CONFIDENTIAL_DEVICE: &str = "confidential-device";
pub const CONFIDENTIAL_BROWSER: &str = "confidential-browser";
/// Anything else: no step to send the user back to.
pub const OTHER: &str = "other";

/// The answers of the Outlook choices, as the steps that depend on them read them.
pub const PERSONAL: &str = "personal";
pub const WORK: &str = "work";
pub const BROWSER: &str = "browser";
pub const DEVICE_CODE: &str = "device-code";

const NAME_STEP_WHY: &str = "What neomail calls the account, as in `neomail sync --account NAME`.";
const READY_WHY: &str = "The name is free, the config file can take the account and the keyring \
                         opens, before anything else is asked.";
const SIGN_IN_WHY: &str = "The first point where the whole setup is tried: sign in as the \
                           address you gave and allow access to your mail.";
const WRITE_WHY: &str = "The sign-in goes to the keyring and the account to the config file.";

/// Gmail with your own Google OAuth client. Google's pages, by step: projects
/// (<https://cloud.google.com/resource-manager/docs/creating-managing-projects>), scopes
/// (<https://support.google.com/cloud/answer/15549135>), the consent setup
/// (<https://developers.google.com/workspace/guides/configure-oauth-consent>), refresh tokens
/// (<https://developers.google.com/identity/protocols/oauth2>), verification
/// (<https://support.google.com/cloud/answer/13464323>), Advanced Protection
/// (<https://support.google.com/accounts/answer/7539956>) and clients
/// (<https://support.google.com/cloud/answer/15549257>).
pub static GMAIL: Wizard = Wizard {
    id: "gmail",
    steps: &[
        Step {
            id: "project",
            title: "Create a Google Cloud project",
            why: "\
neomail ships no OAuth client, so you create your own, and every client belongs to a project.
Create one (\"neomail\", say) on its own, apart from anything else you run. No billing is
needed. Its project ID can be changed only while you create it.",
            links: &["https://console.cloud.google.com/projectcreate"],
            kind: Kind::Console,
        },
        Step {
            id: "project-id",
            title: "The project's ID",
            why: "\
The Project ID from the project's dashboard, not its name or number. With it, the links in the
next steps open this project; press Enter to skip, then pick the project at the top of each
page.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--project"),
                prompt: "Project ID (Enter to skip): ",
                secret: false,
                default: Some(""),
                clean: clean_project,
            }),
        },
        Step {
            id: "gmail-api",
            title: "Enable the Gmail API",
            why: "\
Click Enable. Google lists only the scopes of enabled APIs when you add scopes under Data
Access, and Gmail's scope is the one neomail reads your mail with.",
            links: &[
                "https://console.cloud.google.com/apis/library/gmail.googleapis.com?project={project-id}",
            ],
            kind: Kind::Console,
        },
        Step {
            id: "branding",
            title: "Set up the Google Auth Platform",
            why: "\
Branding > Get started: an app name only you will see, your address as the support email,
Audience External, your address as the contact, agree to the policy, then Create.
Why External: it lets a personal @gmail.com account sign in; Internal admits only a Google
Workspace organisation's own users.",
            links: &["https://console.developers.google.com/auth/branding?project={project-id}"],
            kind: Kind::Console,
        },
        Step {
            id: "email",
            title: "Your Gmail address",
            why: "The account neomail reads. You add it as a test user next, and sign in with it.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--email"),
                prompt: "Gmail address: ",
                secret: false,
                default: None,
                clean: clean_address,
            }),
        },
        Step {
            id: "test-user",
            title: "Add yourself as a test user",
            why: "\
Audience > Test users > Add users: add the address you just gave, then Save.
Why: while the app's publishing status is Testing, only its test users can sign in.",
            links: &["https://console.developers.google.com/auth/audience?project={project-id}"],
            kind: Kind::Console,
        },
        Step {
            id: "scope",
            title: "Data Access: add the mail scope",
            why: "\
Data Access > Add or Remove Scopes: tick https://mail.google.com/ (or paste it under Manually
add scopes), click Update, then Save.
Why: Gmail's IMAP and SMTP accept OAuth only with this scope.",
            links: &[
                "https://console.developers.google.com/auth/scopes?project={project-id}",
                "https://developers.google.com/workspace/gmail/imap/xoauth2-protocol",
            ],
            kind: Kind::Console,
        },
        Step {
            id: "publishing",
            title: "Testing or published: choose now",
            why: "\
Audience shows the publishing status, Testing for a new app. It decides how long a sign-in lasts:
- Testing: Google ends each sign-in after 7 days, so you run `neomail account login NAME`
  every week. Nothing more to do.
- Published (Audience > Publish app): sign-ins last. Your app is unverified, so Google warns at
  sign-in (Advanced, then Go to the app's name) and caps it at 100 users; an app for your own
  use needs no verification.
Neither works with Advanced Protection: it lets only verified apps reach the account's data.",
            links: &[
                "https://console.developers.google.com/auth/audience?project={project-id}",
                "https://developers.google.com/identity/protocols/oauth2#expiration",
                "https://support.google.com/cloud/answer/13464323",
                "https://support.google.com/accounts/answer/7539956",
            ],
            kind: Kind::Console,
        },
        Step {
            id: "name",
            title: "Name the account",
            why: NAME_STEP_WHY,
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--name"),
                prompt: "Account name in neomail: ",
                secret: false,
                default: Some("gmail"),
                clean: clean_name,
            }),
        },
        Step {
            id: "ready",
            title: "Check that the account can be saved",
            why: READY_WHY,
            links: &[],
            kind: Kind::Check {
                reads: &["name", "email"],
            },
        },
        Step {
            id: "client",
            title: "Create a Desktop app client",
            why: "\
Clients > Create client > Application type: Desktop app, any name, Create. Copy the client ID
and the client secret now: Google shows the secret in full only this once. Lost it? Open the
client and click Add secret.
Why Desktop app: it may send the sign-in back to http://127.0.0.1 on any port, where neomail
waits for it.",
            links: &[
                "https://console.developers.google.com/auth/clients?project={project-id}",
                "https://support.google.com/cloud/answer/15549257",
            ],
            kind: Kind::Console,
        },
        Step {
            id: "client-id",
            title: "Paste the client ID",
            why: "It ends in .apps.googleusercontent.com.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--client-id"),
                prompt: "Client ID: ",
                secret: false,
                default: None,
                clean: clean_google_client_id,
            }),
        },
        Step {
            id: "secret",
            title: "Paste the client secret",
            why: "The secret of the same client. It goes to the keyring with the sign-in.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: None,
                prompt: "Client secret (not shown): ",
                secret: true,
                default: None,
                clean: clean_secret,
            }),
        },
        Step {
            id: "client-check",
            title: "Check the client with Google",
            why: "Google is asked whether it knows this client ID and secret; no one signs in.",
            links: &[],
            kind: Kind::Check {
                reads: &["client-id", "secret"],
            },
        },
        Step {
            id: "sign-in",
            title: "Sign in",
            why: SIGN_IN_WHY,
            links: &[],
            kind: Kind::Check {
                reads: &["email", "client-id", "secret"],
            },
        },
        Step {
            id: "mail-check",
            title: "Check the mail login",
            why: "neomail logs in to Gmail's IMAP, reading nothing, and to its SMTP, sending \
                  nothing, with the new sign-in.",
            links: &[],
            kind: Kind::Check { reads: &["email"] },
        },
        Step {
            id: "write",
            title: "Save the account",
            why: WRITE_WHY,
            links: &[],
            kind: Kind::Commit,
        },
    ],
    mistakes: &[
        Mistake {
            check: "ready",
            kind: NAME_TAKEN,
            step: "name",
            fix: "Choose another name (--name).",
        },
        Mistake {
            check: "client-check",
            kind: UNKNOWN_ID,
            step: "client-id",
            fix: "Google knows no client with this ID. Copy the Client ID of the Desktop app \
                  client you created.",
        },
        Mistake {
            check: "client-check",
            kind: WRONG_SECRET,
            step: "secret",
            fix: "Google knows the client ID but not this secret. Paste the secret of the same \
                  client; if you no longer have it, open the client and click Add secret.",
        },
        Mistake {
            check: "client-check",
            kind: BAD_CLIENT,
            step: "client-id",
            fix: "Google refused this client ID and secret. Copy both from the same Desktop app \
                  client.",
        },
        Mistake {
            check: "sign-in",
            kind: TIMED_OUT,
            step: "test-user",
            fix: "Google shows these mistakes on its own page and never sends them back: check \
                  the steps named above, starting here.",
        },
        Mistake {
            check: "sign-in",
            kind: DECLINED,
            step: "publishing",
            fix: "At the warning that Google hasn't verified the app, go on: it is your own \
                  (Continue, or Advanced, then Go to the app's name). Then allow mail access.",
        },
        Mistake {
            check: "sign-in",
            kind: BAD_CLIENT,
            step: "secret",
            fix: "Google refused the client ID and secret after the sign-in: paste the secret of \
                  the same client.",
        },
        Mistake {
            check: "sign-in",
            kind: REDIRECT_MISMATCH,
            step: "client",
            fix: "The client must be of type Desktop app, which may redirect to 127.0.0.1.",
        },
        Mistake {
            check: "sign-in",
            kind: NO_MAIL,
            step: "scope",
            fix: "Add https://mail.google.com/ under Data Access, and tick mail access if the \
                  consent screen shows a checkbox for it.",
        },
        Mistake {
            check: "sign-in",
            kind: WRONG_ACCOUNT,
            step: "email",
            fix: "Sign in with the address you gave, or give the one you signed in with.",
        },
    ],
};

/// Outlook with your own Microsoft Entra app. Microsoft's pages, by step: a tenant
/// (<https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-create-new-tenant>),
/// registering (<https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app>),
/// the redirect URI (<https://learn.microsoft.com/en-us/entra/identity-platform/how-to-add-redirect-uri>),
/// public client flows (the AADSTS7000218 article, [`super::HINT_7000218`]) and consent
/// (<https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies>,
/// <https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/configure-user-consent>).
pub static OUTLOOK: Wizard = Wizard {
    id: "outlook",
    steps: &[
        Step {
            id: "account-type",
            title: "Personal, or work or school?",
            why: "\
personal: Outlook.com, Hotmail or Live. work: an address your organisation gave you. Each sees
only the steps that apply to it.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--account-type"),
                prompt: "personal or work? ",
                secret: false,
                default: Some(PERSONAL),
                clean: clean_account_type,
            }),
        },
        Step {
            id: "tenant",
            title: "Get a directory: a free Azure account",
            why: "\
An app registration lives in a directory (tenant), and a personal account has none: Microsoft
says you can't register apps until you have one. Sign up for the free Azure account with this
account; Microsoft asks for a phone number and a card. Then sign in at entra.microsoft.com:
a directory name under your name, not a bare GUID, means it worked.",
            links: &[
                "https://azure.microsoft.com/pricing/purchase-options/azure-account",
                "https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-create-new-tenant",
            ],
            kind: Kind::ConsoleIf {
                step: "account-type",
                is: &[PERSONAL],
            },
        },
        Step {
            id: "register",
            title: "Register the app",
            why: "\
At entra.microsoft.com: Entra ID > App registrations > New registration, any name. Under
Supported account types choose Any Entra ID Tenant + Personal Microsoft accounts (not the
suggested Single tenant), then Register.
Why: neomail signs in through Microsoft's common endpoint, which needs that type.",
            links: &[
                "https://entra.microsoft.com",
                "https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app",
            ],
            kind: Kind::Console,
        },
        Step {
            id: "sign-in-method",
            title: "How you will sign in",
            why: "\
browser: in a browser on this machine. device-code: with a code, on another device (chosen
for you over SSH without a display). It decides the next step.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--device-code"),
                prompt: "browser or device-code? ",
                secret: false,
                default: Some(BROWSER),
                clean: clean_sign_in_method,
            }),
        },
        Step {
            id: "redirect",
            title: "Add the redirect URI",
            why: "\
In the app: Authentication > Add Redirect URI > Mobile and desktop applications. Under Custom
redirect URIs enter http://localhost, then Configure.
Why: Microsoft sends the browser back to http://localhost on any port, where neomail waits.
Under Web instead, Microsoft would take the app for a confidential client and refuse it.",
            links: &[
                "https://learn.microsoft.com/en-us/entra/identity-platform/how-to-add-redirect-uri",
            ],
            kind: Kind::ConsoleIf {
                step: "sign-in-method",
                is: &[BROWSER],
            },
        },
        Step {
            id: "public-client",
            title: "Allow public client flows",
            why: "\
In the app: Authentication > Advanced settings: set Allow public client flows to Yes, then Save.
Why: the device code sign-in sends no redirect URI, so this setting alone tells Microsoft the
app is a public client, which has no secret.",
            links: &[
                "https://learn.microsoft.com/en-us/troubleshoot/entra/entra-id/app-integration/confidential-client-application-authentication-error-aadsts7000218",
            ],
            kind: Kind::ConsoleIf {
                step: "sign-in-method",
                is: &[DEVICE_CODE],
            },
        },
        Step {
            id: "consent",
            title: "Your organisation may need to approve the app",
            why: "\
neomail asks to read, change and send your mail. Microsoft's pages disagree on whether you can
allow that yourself: one says users can by default, another that the default for new tenants
keeps users from allowing Mail.Read and Mail.ReadWrite. If the sign-in asks for an admin's
approval, ask your IT admin to approve the app, then sign in again.",
            links: &[
                "https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/configure-user-consent",
                "https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies",
            ],
            kind: Kind::ConsoleIf {
                step: "account-type",
                is: &[WORK],
            },
        },
        Step {
            id: "email",
            title: "Your Outlook address",
            why: "The account neomail reads, and the one you sign in with.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--email"),
                prompt: "Outlook address: ",
                secret: false,
                default: None,
                clean: clean_address,
            }),
        },
        Step {
            id: "name",
            title: "Name the account",
            why: NAME_STEP_WHY,
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--name"),
                prompt: "Account name in neomail: ",
                secret: false,
                default: Some("outlook"),
                clean: clean_name,
            }),
        },
        Step {
            id: "ready",
            title: "Check that the account can be saved",
            why: READY_WHY,
            links: &[],
            kind: Kind::Check {
                reads: &["name", "email"],
            },
        },
        Step {
            id: "client-id",
            title: "Paste the Application (client) ID",
            why: "\
From the app's Overview page: the Application (client) ID, not the Directory (tenant) ID or
the Object ID. There is no secret: neomail runs on your machine, so it cannot keep one.",
            links: &[],
            kind: Kind::Ask(Question {
                flag: Some("--client-id"),
                prompt: "Application (client) ID: ",
                secret: false,
                default: None,
                clean: clean_guid,
            }),
        },
        Step {
            id: "client-check",
            title: "Check the app with Microsoft",
            why: "Microsoft is asked whether it knows this app; no one signs in.",
            links: &[],
            kind: Kind::Check {
                reads: &["client-id"],
            },
        },
        Step {
            id: "sign-in",
            title: "Sign in",
            why: SIGN_IN_WHY,
            links: &[],
            kind: Kind::Check {
                reads: &["account-type", "sign-in-method", "email", "client-id"],
            },
        },
        Step {
            id: "mail-check",
            title: "Check the mailbox",
            why: "neomail reads the INBOX's message count through Microsoft Graph, once.",
            links: &[],
            kind: Kind::Check { reads: &["email"] },
        },
        Step {
            id: "write",
            title: "Save the account",
            why: WRITE_WHY,
            links: &[],
            kind: Kind::Commit,
        },
    ],
    mistakes: &[
        Mistake {
            check: "ready",
            kind: NAME_TAKEN,
            step: "name",
            fix: "Choose another name (--name).",
        },
        Mistake {
            check: "client-check",
            kind: UNKNOWN_ID,
            step: "client-id",
            fix: "Microsoft knows no app with this Application (client) ID. Copy it from the \
                  app's Overview page.",
        },
        Mistake {
            check: "client-check",
            kind: NOT_MULTITENANT,
            step: "register",
            fix: "Register the app again with Any Entra ID Tenant + Personal Microsoft accounts, \
                  and use its client ID.",
        },
        Mistake {
            check: "sign-in",
            kind: TIMED_OUT,
            step: "register",
            fix: "Microsoft shows these mistakes on its own page and never sends them back: \
                  check the steps named above, starting here.",
        },
        Mistake {
            check: "sign-in",
            kind: DECLINED,
            step: "consent",
            fix: "Ask your organisation's admin to approve the app, then sign in again.",
        },
        Mistake {
            check: "sign-in",
            kind: NOT_MULTITENANT,
            step: "register",
            fix: "Register the app again with Any Entra ID Tenant + Personal Microsoft accounts, \
                  and use its client ID.",
        },
        Mistake {
            check: "sign-in",
            kind: WORK_ONLY,
            step: "register",
            fix: "The app admits work or school accounts only. Register it again with Any Entra \
                  ID Tenant + Personal Microsoft accounts, and use its client ID.",
        },
        Mistake {
            check: "sign-in",
            kind: UNKNOWN_ID,
            step: "client-id",
            fix: "Copy the Application (client) ID from the app's Overview page.",
        },
        Mistake {
            check: "sign-in",
            kind: CONFIDENTIAL_DEVICE,
            step: "public-client",
            fix: super::HINT_7000218,
        },
        Mistake {
            check: "sign-in",
            kind: CONFIDENTIAL_BROWSER,
            step: "redirect",
            fix: super::HINT_7000218,
        },
        Mistake {
            check: "sign-in",
            kind: WRONG_ACCOUNT,
            step: "email",
            fix: "Sign in with the address you gave, or give the one you signed in with.",
        },
    ],
};

fn clean_name(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim() {
        "" => Err("The account name is empty."),
        name => Ok(Cleaned::ok(name)),
    }
}

/// Something on each side of one `@`, and no spaces.
fn clean_address(typed: &str) -> Result<Cleaned, &'static str> {
    let address = typed.trim();
    match address.split_once('@') {
        Some((local, domain))
            if !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !address.contains(char::is_whitespace) =>
        {
            Ok(Cleaned::ok(address))
        }
        _ => Err("That is not an address: give the whole address, such as you@example.com."),
    }
}

/// Empty (skipped), or a project ID as Google allows it: 6 to 30 lowercase letters, digits and
/// hyphens, starting with a letter and not ending with a hyphen
/// (<https://cloud.google.com/resource-manager/docs/creating-managing-projects>).
pub fn clean_project(typed: &str) -> Result<Cleaned, &'static str> {
    let id = typed.trim();
    let valid = id.is_empty()
        || ((6..=30).contains(&id.len())
            && id.starts_with(|c: char| c.is_ascii_lowercase())
            && !id.ends_with('-')
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    if valid {
        Ok(Cleaned::ok(id))
    } else {
        Err(
            "That is not a project ID: 6 to 30 lowercase letters, digits and hyphens, starting \
             with a letter. Copy the Project ID, not the project's name or number.",
        )
    }
}

fn clean_google_client_id(typed: &str) -> Result<Cleaned, &'static str> {
    let id = typed.trim();
    if id.is_empty() || id.contains(char::is_whitespace) {
        return Err("Paste the client ID: one word, with no spaces.");
    }
    let warning = (!id.ends_with(".apps.googleusercontent.com"))
        .then_some("a Google client ID ends in .apps.googleusercontent.com.");
    Ok(Cleaned {
        value: id.to_owned(),
        warning,
    })
}

fn clean_secret(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim() {
        "" => Err("The client secret is empty."),
        secret => Ok(Cleaned::ok(secret)),
    }
}

/// A GUID, as Entra shows the Application (client) ID: 8-4-4-4-12 hex digits.
pub fn clean_guid(typed: &str) -> Result<Cleaned, &'static str> {
    let id = typed.trim();
    let groups: Vec<&str> = id.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    if lengths == [8, 4, 4, 4, 12]
        && groups
            .iter()
            .all(|g| g.chars().all(|c| c.is_ascii_hexdigit()))
    {
        Ok(Cleaned::ok(id))
    } else {
        Err("That is not an Application (client) ID, which looks like \
             00000000-0000-0000-0000-000000000000. Copy it from the app's Overview page.")
    }
}

fn clean_account_type(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim().to_ascii_lowercase().as_str() {
        "personal" | "p" => Ok(Cleaned::ok(PERSONAL)),
        "work" | "school" | "w" => Ok(Cleaned::ok(WORK)),
        _ => Err("Type personal or work."),
    }
}

fn clean_sign_in_method(typed: &str) -> Result<Cleaned, &'static str> {
    match typed.trim().to_ascii_lowercase().as_str() {
        "browser" | "b" => Ok(Cleaned::ok(BROWSER)),
        "device-code" | "device" | "code" | "d" => Ok(Cleaned::ok(DEVICE_CODE)),
        _ => Err("Type browser or device-code."),
    }
}
