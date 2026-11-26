class Session
  def load
    @user = fetch
    @user.size
  end
end
