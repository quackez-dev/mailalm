# Domaines contactés par MailALM

MailALM ne contacte que les serveurs des comptes configurés (PLAN.md, section 10.3). Pas de télémétrie, pas de
mise à jour automatique, pas de rapport de plantage, pas d'autoconfiguration par un service tiers.

## Phase 2 : comptes IMAP

| Domaine | Port | Protocole | Quand |
|---|---|---|---|
| `imap.videotron.ca` | 993 (TLS) | IMAP | Compte Vidéotron : lecture, dossiers, changements |
| `smtp.videotron.ca` | 465 (TLS) | SMTP | Compte Vidéotron : envoi |
| `imap.bell.net` | 993 (TLS) | IMAP | Compte Bell : lecture, dossiers, changements |
| `smtphm.sympatico.ca` | 587 (STARTTLS exigé) | SMTP | Compte Bell : envoi |
| serveurs saisis par l'utilisateur | ceux saisis | IMAP, SMTP | Autre compte IMAP |

- Les profils Vidéotron et Bell sont **à confirmer** : les pages d'aide des fournisseurs et les guides tiers ne
  s'accordent pas sur les serveurs d'envoi. Ils sont affichés et modifiables dans la fenêtre d'ajout de compte.
- Toutes les connexions sont chiffrées (TLS, ou STARTTLS exigé). Il n'existe pas d'option sans chiffrement ni
  d'exception de certificat.
- La vérification des certificats utilise les autorités de Mozilla (incluses dans MailALM) et celles de Windows,
  moins les certificats que Windows désigne comme non fiables (magasin `Disallowed`).

## Phase 3 : comptes Hotmail et Outlook.com

| Domaine | Port | Protocole | Quand |
|---|---|---|---|
| `login.microsoftonline.com` | 443 (TLS) | HTTPS (OAuth 2, PKCE) | Connexion à Microsoft : échange du code contre les jetons, puis renouvellement du jeton d'accès (environ toutes les heures) |
| `graph.microsoft.com` | 443 (TLS) | HTTPS (Microsoft Graph v1.0) | Compte Outlook : dossiers, messages, changements, envoi, blocage d'expéditeurs |
| `localhost` (ce PC) | port libre choisi au moment de la connexion | HTTP | Le navigateur y renvoie le code de connexion ; MailALM n'écoute que sur ce PC, et seulement pendant la connexion |

- La page de connexion s'ouvre dans le **navigateur par défaut** : le mot de passe Microsoft est tapé là, jamais
  dans MailALM. Ce que le navigateur charge sur cette page relève de Microsoft, pas de MailALM.
- MailALM garde seulement le jeton de renouvellement, dans le Gestionnaire d'identifiants de Windows. Le jeton
  d'accès reste en mémoire.
- Les mêmes vérifications de certificat que pour IMAP s'appliquent à ces connexions.
- MailALM ne suit aucune redirection HTTP, et ne suit un lien de pagination de Graph que s'il pointe vers
  `https://graph.microsoft.com/`.

## Phase 4 : comptes Gmail

| Domaine | Port | Protocole | Quand |
|---|---|---|---|
| `accounts.google.com` | 443 (TLS) | HTTPS | Page de connexion de Google, ouverte dans le **navigateur** (MailALM ne la contacte pas lui-même) |
| `oauth2.googleapis.com` | 443 (TLS) | HTTPS (OAuth 2, PKCE) | Échange du code contre les jetons, puis renouvellement du jeton d'accès (environ toutes les heures) |
| `gmail.googleapis.com` | 443 (TLS) | HTTPS (API Gmail v1) | Compte Gmail : libellés, messages, changements, envoi, filtres (expéditeurs bloqués ou autorisés) |
| `127.0.0.1` (ce PC) | port libre choisi au moment de la connexion | HTTP | Le navigateur y renvoie le code de connexion ; MailALM n'écoute que sur ce PC, et seulement pendant la connexion |

- Comme pour Microsoft, le mot de passe Google est tapé dans le navigateur, jamais dans MailALM, et seul le jeton
  de renouvellement est gardé, dans le Gestionnaire d'identifiants de Windows.
- Permissions demandées : `gmail.modify` et `gmail.settings.basic` (et `openid email profile` pour savoir quel
  compte s'est connecté). Pas l'accès complet : MailALM ne peut pas effacer un message Gmail pour de bon.
- L'API Gmail ne donne aucun lien à suivre (la pagination passe par des jetons) : MailALM ne contacte que les
  adresses ci-dessus.

## Phases suivantes

L'agenda, les tâches et les contacts (phase 6) ajouteront les API Google Agenda, Tasks et People, et l'agenda de
Graph : la liste sera complétée à leur arrivée.
