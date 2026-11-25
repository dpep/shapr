class Store < Base
  def save(record, options)
    super(options, record)
  end
end
