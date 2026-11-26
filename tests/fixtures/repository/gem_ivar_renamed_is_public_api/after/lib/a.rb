class Session
  def load
    @account = fetch
    @account.size
  end
end
