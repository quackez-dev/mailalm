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

## Phase 5 : pièces jointes

Aucun nouveau domaine.

- La revue, l'aperçu (images, pages PDF, texte), l'enregistrement et la copie ouverte avec Windows se font à partir
  du message déjà en cache : rien n'est téléchargé, et un PDF prévisualisé ne charge rien (hayro ne suit ni lien
  ni ressource externe).
- « Ouvrir avec Windows » remet une copie au programme que Windows associe à son extension. Ce que ce programme
  contacte ensuite relève de lui, pas de MailALM.
- Envoi depuis un compte Outlook : Graph `sendMail` seulement, comme en phase 3, limité à 3 Mo par message. Les
  sessions de téléversement de Graph (`createUploadSession`), qui renverraient vers une autre adresse, ne sont pas
  utilisées.

## Phase 6 : agenda, tâches et contacts

| Domaine | Port | Protocole | Quand |
|---|---|---|---|
| `www.googleapis.com` | 443 (TLS) | HTTPS (API Google Agenda v3, chemin `/calendar/v3`) | Compte Gmail : agendas, événements, invitations (réponses), création de liens Google Meet |
| `tasks.googleapis.com` | 443 (TLS) | HTTPS (API Google Tasks v1) | Compte Gmail : listes de tâches et tâches |
| `people.googleapis.com` | 443 (TLS) | HTTPS (API People v1), lecture seule | Compte Gmail : contacts, pour compléter les adresses |
| `graph.microsoft.com` | 443 (TLS) | HTTPS (Microsoft Graph v1.0) | Compte Outlook : en plus du courriel, agendas et événements, réponses aux invitations, réunions Teams, tâches Microsoft To Do, contacts |

- Aucun nouveau domaine pour les comptes Outlook : Graph sert aussi l'agenda (`/me/calendars`, `/me/events`), To
  Do (`/me/todo/lists`) et les contacts (`/me/contacts`), sur `graph.microsoft.com`.
- Permissions supplémentaires demandées :
  - Microsoft : `Calendars.ReadWrite`, `Tasks.ReadWrite`, `Contacts.ReadWrite` (déléguées) ;
  - Google : `calendar`, `tasks`, `contacts.readonly` (les contacts Google ne servent qu'à l'autocomplétion :
    MailALM ne les modifie pas).
  Un compte connecté avant la phase 6 doit se reconnecter pour les accorder.
- Les liens Meet et Teams sont créés par Google et Microsoft eux-mêmes, sur leurs serveurs. MailALM garde le lien
  reçu pour l'afficher ; il ne l'ouvre jamais seul.
- Comme pour l'API Gmail, les API Google donnent des jetons de pagination, jamais des liens à suivre. Les liens de
  pagination de Graph ne sont suivis que s'ils pointent vers `https://graph.microsoft.com/`.
- Comptes IMAP (Vidéotron, Bell) : les invitations partent par courriel (fichier `.ics`), sur les serveurs SMTP
  déjà listés. Aucun autre domaine.

## Phase 7 : Antidote

| Adresse | Port | Protocole | Quand |
|---|---|---|---|
| `127.0.0.1`, sinon `::1` (ce PC) | celui que donne `AgentConnectixConsole --api` | WebSocket en clair, local | L'utilisateur ouvre le correcteur, les dictionnaires ou les guides d'Antidote, ou l'Anti-Oups! avant l'envoi |

- Antidote est un **programme local** : MailALM lit son dossier dans le registre
  (`HKLM\SOFTWARE\Druide informatique inc.\Connectix`, valeur `DossierConnectix`), lance
  `AgentConnectixConsole.exe --api` (sans fenêtre ni interpréteur de commandes) pour connaître le port, puis se
  connecte à ce port **sur ce PC seulement**. Le code ne peut pas viser un autre hôte.
- Le texte envoyé (zones à corriger, objet, corps et destinataires pour l'Anti-Oups!) va à l'Antidote installé sur
  le PC. **Aucun texte ne sort du PC** par MailALM ; ce qu'Antidote fait ensuite relève d'Antidote.

## Phase 9 : serveur MCP

| Adresse | Port | Protocole | Quand |
|---|---|---|---|
| `127.0.0.1` (ce PC) | port libre choisi par le système au démarrage du canal, écrit dans `%LOCALAPPDATA%\MailALM\control-port` | TCP local, une ligne JSON par requête et par réponse | Seulement si l'accès MCP est **activé** dans les réglages de MailALM (il est **désactivé par défaut**) : `mailalm-mcp`, lancé par l'agent, y transmet chaque appel d'outil |

- **Aucun nouveau domaine.** Le canal ne sort pas du PC : MailALM n'écoute que sur `127.0.0.1` (jamais `0.0.0.0`
  ni `::1`) et ferme toute connexion dont l'adresse n'est pas `127.0.0.1`. `mailalm-mcp` ne se connecte qu'à
  `127.0.0.1`.
- **Clé** : à chaque démarrage du canal, MailALM tire 32 octets aléatoires (`ring`) et les écrit en hexadécimal
  dans `%LOCALAPPDATA%\MailALM\control-key`, dans le profil de l'utilisateur. Chaque requête porte la clé ; elle
  est comparée en temps constant, et une clé fausse ou absente est refusée et journalisée. Les deux fichiers sont
  effacés quand l'accès MCP est désactivé ou que MailALM se ferme. Un fichier de clé qui n'a pas exactement
  64 caractères hexadécimaux est refusé.
- **Limites** : une requête par connexion, une ligne de 4 Mio au plus, envoyée en 10 s au plus ; 8 connexions à la
  fois au plus. Une requête que MailALM n'a pas prise en 30 s n'est jamais exécutée ; une action sensible que
  l'utilisateur n'a pas confirmée en 5 minutes non plus.
- **Ce que l'agent reçoit** : le texte des courriels (jamais le HTML ni le contenu des pièces jointes), toujours
  dans un champ `external_content` précédé d'un avertissement : c'est du texte écrit par des tiers, pas des
  consignes. Les comptes que l'utilisateur rend invisibles au MCP sont refusés, et retirés des réponses qui
  couvrent tous les comptes.
- **Ce que l'agent peut faire sans confirmation** : lire, créer un brouillon, classer, marquer, ajouter une
  catégorie, créer un événement provisoire sans invités (aucune invitation ne part) ou une tâche. **Avec
  confirmation dans MailALM** : envoyer, supprimer, bloquer un expéditeur, répondre à une invitation, changer une
  règle (jamais de transfert), et appliquer un tri proposé (ligne par ligne).
- Ce que l'agent fait ensuite des données reçues relève de l'agent et de son fournisseur, pas de MailALM : c'est
  pourquoi l'accès est désactivé par défaut et réglable compte par compte.
