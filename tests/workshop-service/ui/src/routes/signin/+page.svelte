<script lang="ts">
  function submit(e: Event) {
    e.preventDefault();
    const fd = new FormData(e.target as HTMLFormElement);
    document.cookie = `x-user-id=${fd.get('userId')}; path=/`;
    document.cookie = `x-role=${fd.get('role')}; path=/`;
    location.href = '/protected';
  }
</script>

<h1>Sign in (dev)</h1>
<p>Sets cookies for GraphQL identity headers (the-website Auth.js pattern, simplified).</p>
<form onsubmit={submit}>
  <label>User id <input name="userId" value="customer-1" /></label>
  <label>
    Role
    <select name="role">
      <option value="customer">customer</option>
      <option value="maker">maker</option>
      <option value="admin">admin</option>
    </select>
  </label>
  <button type="submit">Continue</button>
</form>
