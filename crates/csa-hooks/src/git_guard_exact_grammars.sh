is_exact_git_version_grammar() {
  [ "$#" -eq 1 ] || return 1
  case "${1}" in
    version|--version) return 0 ;;
    *) return 1 ;;
  esac
}

is_exact_git_init_grammar() {
  [ "${1:-}" = "init" ] || return 1
  case "$#" in
    1) return 0 ;;
    2)
      case "${2}" in
        -q|--quiet) return 0 ;;
      esac
      ;;
  esac
  return 1
}

is_exact_git_var_identity_grammar() {
  [ "$#" -eq 2 ] || return 1
  [ "${1}" = "var" ] || return 1
  case "${2}" in
    GIT_AUTHOR_IDENT|GIT_COMMITTER_IDENT) return 0 ;;
    *) return 1 ;;
  esac
}

is_exact_git_c_local_grammar() {
  [ "$#" -ge 3 ] || return 1
  [ "${1}" = "-C" ] || return 1
  has_control_bytes "${2}" && return 1
  case "${2}" in
    *://*|?*:*) return 1 ;;
  esac
  (canonical_directory "${2}" >/dev/null) || return 1
  git_c_command="${3}"
  shift 3
  case "${git_c_command}" in
    init|config|add|commit|status|diff|diff-tree|rev-parse|log|show|checkout) ;;
    *) return 1 ;;
  esac
  for fixture_arg do
    has_control_bytes "${fixture_arg}" && return 1
    case "${fixture_arg}" in
      --|-C|-c|--config|--config=*|--config-env|--config-env=*|--git-dir|--git-dir=*|--work-tree|--work-tree=*|--namespace|--namespace=*|--exec-path|--exec-path=*|--super-prefix|--super-prefix=*|--upload-pack|--upload-pack=*|*:refs/*|*://*|[Aa][Ll][Ii][Aa][Ss].*|remote.*|url.*|core.hooks[Pp]ath*|hooks[Pp]ath*) return 1 ;;
    esac
  done
  case "${git_c_command}" in
    init)
      case "$#" in
        0) ;;
        1) case "${1}" in -q|--quiet) ;; *) return 1 ;; esac ;;
        2)
          case "${1}" in -q|--quiet) ;; *) return 1 ;; esac
          case "${2}" in ""|.|..|-*|*/*|*:*) return 1 ;; esac
          ;;
        *) return 1 ;;
      esac
      ;;
    config)
      [ "$#" -eq 2 ] || return 1
      case "${1}" in user.name|user.email) ;; *) return 1 ;; esac
      ;;
    add)
      [ "$#" -ge 1 ] || return 1
      for fixture_arg do case "${fixture_arg}" in -*) return 1 ;; esac; done
      ;;
    commit)
      case "$#" in
        2) case "${1}" in -m|--message|-qm) ;; *) return 1 ;; esac ;;
        3)
          [ "${1}" = "-q" ] || return 1
          case "${2}" in -m|--message) ;; *) return 1 ;; esac
          ;;
        *) return 1 ;;
      esac
      ;;
    status)
      case "$#" in
        0) ;;
        1) case "${1}" in --short|--porcelain|--porcelain=v1) ;; *) return 1 ;; esac ;;
        *) return 1 ;;
      esac
      ;;
    diff)
      case "$#" in
        0) ;;
        1) [ "${1}" = "--quiet" ] || return 1 ;;
        *) return 1 ;;
      esac
      ;;
    diff-tree)
      [ "$#" -eq 4 ] \
        && [ "${1}" = "--no-commit-id" ] \
        && [ "${2}" = "--name-only" ] \
        && [ "${3}" = "-r" ] \
        && [ "${4}" = "HEAD" ] || return 1
      ;;
    rev-parse)
      case "$#" in
        1) case "${1}" in HEAD|--show-toplevel) ;; *) return 1 ;; esac ;;
        2)
          case "${1}" in --verify|--short) ;; *) return 1 ;; esac
          case "${2}" in ""|-*) return 1 ;; esac
          ;;
        *) return 1 ;;
      esac
      ;;
    log)
      [ "$#" -eq 2 ] && [ "${1}" = "-1" ] || return 1
      case "${2}" in --format=?*) ;; *) return 1 ;; esac
      ;;
    show) [ "$#" -eq 1 ] && case "${1}" in ""|-*) return 1 ;; esac || return 1 ;;
    checkout)
      [ "$#" -eq 2 ] && [ "${1}" = "-b" ] || return 1
      case "${2}" in ""|-*|*:*) return 1 ;; esac
      ;;
  esac
  return 0
}
