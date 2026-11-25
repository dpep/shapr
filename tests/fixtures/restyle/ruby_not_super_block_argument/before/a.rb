class Store < Base
  def save(record, &block)
    super(record, &other)
  end
end
