class Fetcher
  def fetch(key, retries: 3)
    binding.local_variable_get(:retries)
  end

  def go(x)
    fetch(x, retries: 3)
  end
end
