class Fetcher
  def go(x)
    fetch(x, retries: 3)
  end
end
