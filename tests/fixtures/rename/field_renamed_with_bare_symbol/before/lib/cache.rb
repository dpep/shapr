class Cache
  MEMOISED = %i[@entries @loaded]
  FLAGS = %i[loaded].freeze.freeze

  def initialize
    @entries = {}
  end

  def first
    @entries.values.first
  end

  def size
    @entries.size
  end

  def count
    @entries.count
  end

  def load
    return if @loaded
    @loaded = true
  end
end
