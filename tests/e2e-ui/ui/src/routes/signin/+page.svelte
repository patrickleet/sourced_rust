<script lang="ts">
  import { writeSession } from '$lib/session';

  function submit(e: Event) {
    e.preventDefault();
    const fd = new FormData(e.target as HTMLFormElement);
    writeSession({
      userId: String(fd.get('userId') || 'alice'),
      role: String(fd.get('role') || 'user'),
    });
    location.href = '/';
  }
</script>

<h1>Switch user</h1>
<p>
  Dev session cookies (<code>x-user-id</code> / <code>x-role</code>) — same trust model as
  DevHeaders identity on the API. Production would use Auth.js / OIDC like the-website.
</p>
<form onsubmit={submit} style="display: grid; gap: 0.75rem; max-width: 20rem">
  <label>
    User id
    <input name="userId" value="alice" style="width: 100%" />
  </label>
  <label>
    Role
    <select name="role">
      <option value="user">user</option>
      <option value="admin">admin</option>
    </select>
  </label>
  <button type="submit">Continue</button>
</form>
