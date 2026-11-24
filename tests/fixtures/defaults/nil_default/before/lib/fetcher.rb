class Fetcher
  def find(key, scope: nil)
    [key, scope]
  end

  def go(x)
    find(x, scope: nil)
  end
end
